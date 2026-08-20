(() => {
  "use strict";

  const UUIDS = Object.freeze({
    service: "d91714ef-28b9-4f91-ba16-f0d9a604f112",
    command: "f4bf14a6-c7d5-4b6d-8aa8-df1a7c83adcb",
    response: "b41e6675-a329-40e0-aa01-44d2f444babe",
  });
  const COLORS = Object.freeze([
    "#00c2ff", "#ffb000", "#ff5c8a", "#7bd88f", "#b392f0", "#ff7b54", "#58d6c7", "#e5d85c",
  ]);
  const sessions = new Map();

  class GdxBrowserError extends Error {
    constructor(code, message, retryable = false) {
      super(message);
      this.name = "GdxBrowserError";
      this.code = code;
      this.retryable = Boolean(retryable);
    }
  }

  function supportStatus() {
    if (!window.isSecureContext) {
      return { supported: false, reason: "Go Direct browser input requires HTTPS or localhost." };
    }
    if (!navigator.bluetooth?.requestDevice) {
      return { supported: false, reason: "This browser does not expose Web Bluetooth." };
    }
    return { supported: true, reason: "Choose a GDX device; Polar Stream verifies a GDX-RB respiration belt before streaming." };
  }

  function sourceSlot() {
    for (let index = 0; index < COLORS.length; index += 1) {
      const id = `browser-source-${index + 1}`;
      if (!sessions.has(id)) {
        return {
          id,
          slot: id,
          label: `Browser source ${index + 1}`,
          color: COLORS[index],
          inputKind: "vernierGoDirect",
        };
      }
    }
    throw new GdxBrowserError("INPUT_CAPACITY_REACHED", "At most eight browser Bluetooth sources are supported.", true);
  }

  function packet(counter, id, payload = []) {
    counter.value = (counter.value - 1) & 0xff;
    const bytes = Uint8Array.from([0x58, payload.length + 5, counter.value, 0, id, ...payload]);
    bytes[3] = bytes.reduce((sum, value) => (sum + value) & 0xff, 0);
    return bytes;
  }

  function setPeriodPacket(counter, periodUs) {
    const bytes = new Uint8Array(10);
    bytes[0] = 0xff;
    bytes[1] = 0;
    new DataView(bytes.buffer).setUint32(2, periodUs, true);
    return packet(counter, 0x1b, bytes);
  }

  function startPacket(counter, sensorNumber) {
    const payload = new Uint8Array(14);
    payload[0] = 0xff;
    payload[1] = 1;
    new DataView(payload.buffer).setUint32(2, 2 ** sensorNumber, true);
    return packet(counter, 0x18, payload);
  }

  function fixedText(bytes, offset, length) {
    const end = Math.min(bytes.length, offset + length);
    return new TextDecoder().decode(bytes.slice(offset, end)).replaceAll("\0", "").trim();
  }

  function classifyDevice(advertisedName, orderCode, description) {
    const identity = `${advertisedName || ""} ${orderCode || ""} ${description || ""}`.toUpperCase();
    if (identity.includes("GDX-RB") || identity.includes("RESPIRATION BELT")) {
      return { code: "GDX-RB", name: "Go Direct Respiration Belt", supported: true };
    }
    return { code: "GDX-UNKNOWN", name: "Unknown Go Direct model", supported: false };
  }

  function parseDeviceInfo(frame, advertisedName) {
    if (frame.length < 158) {
      throw new GdxBrowserError("GDX_DEVICE_INFO_TRUNCATED", "Go Direct returned incomplete device identity metadata.");
    }
    const orderCode = fixedText(frame, 6, 16);
    const name = fixedText(frame, 38, 32) || advertisedName || "Vernier Go Direct";
    const description = fixedText(frame, 94, 64);
    return { orderCode, name, description, model: classifyDevice(advertisedName, orderCode, description) };
  }

  function parseSensorInfo(frame) {
    if (frame.length < 154) {
      throw new GdxBrowserError("GDX_SENSOR_INFO_TRUNCATED", "Go Direct returned incomplete sensor metadata.");
    }
    const view = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
    return {
      number: frame[6],
      numericType: frame[12],
      samplingMode: frame[13],
      description: fixedText(frame, 14, 60),
      unit: fixedText(frame, 74, 32),
      minimumPeriodUs: view.getUint32(130, true),
      maximumPeriodUs: Number(view.getBigUint64(134, true)),
      typicalPeriodUs: view.getUint32(142, true),
      periodGranularityUs: view.getUint32(146, true),
    };
  }

  function availableSensorNumbers(frame) {
    if (frame.length < 10) {
      throw new GdxBrowserError("GDX_SENSOR_MASK_TRUNCATED", "Go Direct returned an incomplete sensor inventory.");
    }
    const mask = new DataView(frame.buffer, frame.byteOffset, frame.byteLength).getUint32(6, true);
    return Array.from({ length: 32 }, (_, number) => number).filter((number) => mask & (2 ** number));
  }

  function respirationPeriod(sensor, requestedPeriodUs = 100000) {
    const inRange = requestedPeriodUs >= (sensor.minimumPeriodUs || 0)
      && (!sensor.maximumPeriodUs || requestedPeriodUs <= sensor.maximumPeriodUs);
    const onGranularity = !sensor.periodGranularityUs || requestedPeriodUs % sensor.periodGranularityUs === 0;
    if (requestedPeriodUs >= 1000 && requestedPeriodUs <= 60000000 && inRange && onGranularity) {
      return requestedPeriodUs;
    }
    if (sensor.typicalPeriodUs >= 1000 && sensor.typicalPeriodUs <= 60000000) return sensor.typicalPeriodUs;
    throw new GdxBrowserError("GDX_INVALID_SAMPLE_PERIOD", "The respiration-belt channel did not report a usable sample period.");
  }

  class BrowserSession {
    constructor(device, source, callback) {
      this.device = device;
      this.source = source;
      this.callback = callback;
      this.counter = { value: 0xff };
      this.accumulator = [];
      this.responseWaiters = [];
      this.sequence = 0;
      this.periodUs = 100000;
      this.sensorNumber = 1;
      this.connectedEventSent = false;
      this.startedAt = performance.now();
      this.onValue = this.onValue.bind(this);
      this.onDisconnected = this.onDisconnected.bind(this);
    }

    emit(event) {
      this.callback({ ...event, source: this.source, transport: "web-bluetooth" });
    }

    async open() {
      this.emit({ kind: "status", phase: "connecting", message: `Opening ${this.device.name || "Vernier Go Direct"}` });
      this.device.addEventListener("gattserverdisconnected", this.onDisconnected);
      const server = await this.device.gatt.connect();
      const service = await server.getPrimaryService(UUIDS.service);
      this.command = await service.getCharacteristic(UUIDS.command);
      this.response = await service.getCharacteristic(UUIDS.response);
      this.response.addEventListener("characteristicvaluechanged", this.onValue);
      await this.response.startNotifications();
      this.emit({ kind: "status", phase: "initializing", message: "Negotiating Go Direct protocol" });
      const initialize = [
        0xa5, 0x4a, 0x06, 0x49, 0x07, 0x48, 0x08, 0x47, 0x09, 0x46,
        0x0a, 0x45, 0x0b, 0x44, 0x0c, 0x43, 0x0d, 0x42, 0x0e, 0x41,
      ];
      for (const command of [packet(this.counter, 0x1a, initialize), packet(this.counter, 0x10)]) {
        await this.writeAndWait(command);
      }
      const deviceInfo = parseDeviceInfo(
        await this.writeAndWait(packet(this.counter, 0x55)),
        this.device.name,
      );
      if (!deviceInfo.model.supported) {
        throw new GdxBrowserError(
          "GDX_UNSUPPORTED_MODEL",
          `Detected ${deviceInfo.orderCode || deviceInfo.name}, but this input accepts only the GDX-RB respiration belt.`,
          true,
        );
      }
      const available = await this.writeAndWait(packet(this.counter, 0x51));
      const sensors = [];
      for (const number of availableSensorNumbers(available)) {
        try {
          sensors.push(parseSensorInfo(await this.writeAndWait(packet(this.counter, 0x50, [number]))));
        } catch (error) {
          this.emit({ kind: "error", code: error.code || "GDX_SENSOR_INFO_INVALID", message: error.message });
        }
      }
      const candidates = sensors.filter((sensor) => (
        sensor.number === 1
        && sensor.samplingMode === 0
        && sensor.description.toLowerCase() === "force"
        && sensor.unit.toLowerCase() === "n"
      ));
      if (candidates.length !== 1) {
        throw new GdxBrowserError(
          candidates.length ? "GDX_RESPIRATION_CHANNEL_AMBIGUOUS" : "GDX_RESPIRATION_CHANNEL_MISSING",
          "The GDX-RB did not expose exactly one periodic Force (N) channel at channel 1.",
        );
      }
      const sensor = candidates[0];
      this.sensorNumber = sensor.number;
      this.periodUs = respirationPeriod(sensor, this.periodUs);
      this.sensorName = sensor.description;
      this.sensorUnit = sensor.unit;
      this.deviceModel = deviceInfo.model.code;
      this.source = {
        ...this.source,
        deviceModel: this.deviceModel,
        sensorNumber: this.sensorNumber,
        sensorName: this.sensorName,
        sensorUnit: this.sensorUnit,
        samplePeriodUs: this.periodUs,
      };
      this.emit({
        kind: "status",
        phase: "identified",
        message: `${this.deviceModel} · ${this.sensorName} (${this.sensorUnit}) · channel ${this.sensorNumber} · ${(1000000 / this.periodUs).toFixed(1)} Hz`,
      });
      await this.writeAndWait(setPeriodPacket(this.counter, this.periodUs));
      await this.writeAndWait(startPacket(this.counter, this.sensorNumber));
      return this.source;
    }

    async writeAndWait(command) {
      const response = new Promise((resolve, reject) => {
        const timeout = window.setTimeout(() => {
          this.responseWaiters = this.responseWaiters.filter((waiter) => waiter.resolve !== resolve);
          reject(new GdxBrowserError("GDX_RESPONSE_TIMEOUT", "Go Direct command response timed out.", true));
        }, 8000);
        this.responseWaiters.push({ resolve, timeout, id: command[4], counter: command[2] });
      });
      await this.write(command);
      return response;
    }

    async write(command) {
      for (let offset = 0; offset < command.length; offset += 20) {
        const chunk = command.slice(offset, offset + 20);
        if (this.command.writeValueWithResponse) await this.command.writeValueWithResponse(chunk);
        else await this.command.writeValue(chunk);
      }
    }

    onValue(event) {
      const view = event.target.value;
      const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
      this.accumulator.push(...bytes);
      while (this.accumulator.length >= 2) {
        const length = this.accumulator[1];
        if (length < 5 || length > 255) {
          this.accumulator = [];
          this.emit({ kind: "error", code: "GDX_INVALID_FRAME", message: "Go Direct sent an invalid frame length." });
          return;
        }
        if (this.accumulator.length < length) return;
        this.decode(Uint8Array.from(this.accumulator.splice(0, length)));
      }
    }

    decode(frame) {
      if (frame[0] !== 0x20) {
        const waiterIndex = this.responseWaiters.findIndex((candidate) => (
          candidate.id === frame[4] && candidate.counter === frame[5]
        ));
        const waiter = waiterIndex >= 0 ? this.responseWaiters.splice(waiterIndex, 1)[0] : null;
        if (waiter) {
          clearTimeout(waiter.timeout);
          waiter.resolve(frame);
        }
        return;
      }
      const subtype = frame[4];
      let sensorNumber;
      let count;
      let offset;
      if (subtype === 0x06) {
        const mask = frame[5] | (frame[6] << 8);
        const selected = [];
        for (let number = 0; number < 16; number += 1) if (mask & (2 ** number)) selected.push(number);
        count = frame[7];
        offset = 9;
        const values = selected.map((number) => ({ number, values: [] }));
        const data = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
        for (let sample = 0; sample < count; sample += 1) {
          for (let sensorIndex = 0; sensorIndex < values.length && offset + 4 <= frame.length; sensorIndex += 1, offset += 4) {
            values[sensorIndex].values.push(data.getFloat32(offset, true));
          }
        }
        for (const sensor of values) this.emitSamples(sensor.number, sensor.values);
        return;
      }
      if (![0x08, 0x09, 0x0a, 0x0b].includes(subtype)) return;
      sensorNumber = frame[6];
      count = frame[7];
      offset = 8;
      const data = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
      const values = [];
      for (let index = 0; index < count && offset + 4 <= frame.length; index += 1, offset += 4) {
        values.push(subtype === 0x09 || subtype === 0x0b ? data.getInt32(offset, true) : data.getFloat32(offset, true));
      }
      this.emitSamples(sensorNumber, values);
    }

    emitSamples(sensorNumber, values) {
      if (sensorNumber !== this.sensorNumber || !values.length) return;
      if (!this.connectedEventSent) {
        this.connectedEventSent = true;
        this.emit({
          kind: "connection", connected: true, streaming: true,
          deviceName: this.device.name || "Vernier Go Direct", batteryPercent: null,
          deviceModel: this.deviceModel, sensorNumber: this.sensorNumber,
          sensorName: this.sensorName, sensorUnit: this.sensorUnit, samplePeriodUs: this.periodUs,
          message: `Verified ${this.sensorName} (${this.sensorUnit}) on channel ${this.sensorNumber} is streaming at ${(1000000 / this.periodUs).toFixed(1)} Hz`,
        });
      }
      const sequence = this.sequence;
      this.sequence += values.length;
      this.emit({
        kind: "force", sensorNumber, sensorName: this.sensorName, sensorUnit: this.sensorUnit,
        hostReceiveTimestampNs: Math.round((performance.now() - this.startedAt) * 1000000),
        samplePeriodUs: this.periodUs, sequence, values,
        droppedBefore: 0, decodeLatencyNs: 0,
      });
    }

    async close({ notify = true } = {}) {
      try { await this.write(packet(this.counter, 0x19, [0xff, 0, 0xff, 0xff, 0xff, 0xff])); } catch (_) {}
      try { await this.response?.stopNotifications(); } catch (_) {}
      this.response?.removeEventListener("characteristicvaluechanged", this.onValue);
      this.device.removeEventListener("gattserverdisconnected", this.onDisconnected);
      if (this.device.gatt?.connected) this.device.gatt.disconnect();
      sessions.delete(this.source.id);
      if (notify) this.emit({
        kind: "connection", connected: false, streaming: false,
        deviceName: this.device.name || "Vernier Go Direct", batteryPercent: null,
        message: "Disconnected",
      });
    }

    onDisconnected() {
      void this.close({ notify: true });
    }
  }

  async function connect(callback) {
    const status = supportStatus();
    if (!status.supported) throw new GdxBrowserError("WEB_BLUETOOTH_UNAVAILABLE", status.reason);
    let device;
    try {
      device = await navigator.bluetooth.requestDevice({
        filters: [{ namePrefix: "GDX" }],
        optionalServices: [UUIDS.service],
      });
    } catch (error) {
      if (error?.name === "NotFoundError") {
        throw new GdxBrowserError("BLUETOOTH_CHOOSER_CANCELLED", "No Go Direct device was selected.", true);
      }
      throw error;
    }
    if ([...sessions.values()].some((session) => session.device.id === device.id)) {
      throw new GdxBrowserError("DEVICE_ALREADY_CONNECTED", "That Go Direct device is already connected.", true);
    }
    const source = sourceSlot();
    const session = new BrowserSession(device, source, callback);
    sessions.set(source.id, session);
    try {
      await session.open();
      return session.source;
    } catch (error) {
      await session.close({ notify: false });
      throw error;
    }
  }

  async function disconnect(sourceId) {
    if (sourceId) return sessions.get(sourceId)?.close();
    await Promise.all([...sessions.values()].map((session) => session.close()));
  }

  window.VernierWebBluetooth = Object.freeze({
    moduleId: "web-bluetooth-vernier-gdx",
    supportStatus,
    connect,
    disconnect,
    activeSources: () => [...sessions.keys()],
  });
})();
