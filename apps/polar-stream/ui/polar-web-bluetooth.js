(() => {
  "use strict";

  const UUIDS = Object.freeze({
    heartRateService: "0000180d-0000-1000-8000-00805f9b34fb",
    heartRateMeasurement: "00002a37-0000-1000-8000-00805f9b34fb",
    batteryService: "0000180f-0000-1000-8000-00805f9b34fb",
    batteryLevel: "00002a19-0000-1000-8000-00805f9b34fb",
    pmdService: "fb005c80-02e7-f387-1cad-8acd2d8df0c8",
    pmdControl: "fb005c81-02e7-f387-1cad-8acd2d8df0c8",
    pmdData: "fb005c82-02e7-f387-1cad-8acd2d8df0c8",
  });
  const MEASUREMENTS = Object.freeze({ ecg: 0x00, accelerometer: 0x02 });
  const COMMANDS = Object.freeze({
    startEcg: Object.freeze([0x02, 0x00, 0x00, 0x01, 130, 0, 0x01, 0x01, 14, 0]),
    startAccelerometer: Object.freeze([0x02, 0x02, 0x02, 0x01, 8, 0, 0x00, 0x01, 200, 0, 0x01, 0x01, 16, 0]),
    stopEcg: Object.freeze([0x03, 0x00]),
    stopAccelerometer: Object.freeze([0x03, 0x02]),
  });
  const SAMPLE_RATE_HZ = 200;

  class WebBluetoothError extends Error {
    constructor(code, message, retryable = false) {
      super(message);
      this.name = "WebBluetoothError";
      this.code = code;
      this.retryable = Boolean(retryable);
    }
  }

  function asBytes(value) {
    if (value instanceof Uint8Array) return value;
    if (value instanceof DataView) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    return Uint8Array.from(value || []);
  }

  function readUnsigned64Le(bytes, offset) {
    let result = 0n;
    for (let index = 7; index >= 0; index -= 1) {
      result = (result << 8n) | BigInt(bytes[offset + index]);
    }
    return result;
  }

  function readSigned16Le(bytes, offset) {
    const raw = bytes[offset] | (bytes[offset + 1] << 8);
    return raw & 0x8000 ? raw - 0x10000 : raw;
  }

  function readSigned24Le(bytes, offset) {
    const raw = bytes[offset] | (bytes[offset + 1] << 8) | (bytes[offset + 2] << 16);
    return raw & 0x800000 ? raw - 0x1000000 : raw;
  }

  function readSignedBits(bytes, bitOffset, width) {
    let value = 0;
    for (let shift = 0; shift < width; shift += 1) {
      const absolute = bitOffset + shift;
      value += ((bytes[Math.floor(absolute / 8)] >> (absolute % 8)) & 1) * (2 ** shift);
    }
    return value >= 2 ** (width - 1) ? value - 2 ** width : value;
  }

  function decodeCompressedAccelerometer(payload) {
    if (payload.length < 8) {
      throw new WebBluetoothError("PMD_INVALID_ACC", "Compressed accelerometer payload has an invalid length.");
    }
    let xMg = readSigned16Le(payload, 0);
    let yMg = readSigned16Le(payload, 2);
    let zMg = readSigned16Le(payload, 4);
    const samples = [{ xMg, yMg, zMg }];
    let offset = 6;
    while (offset < payload.length) {
      if (offset + 2 > payload.length) {
        throw new WebBluetoothError("PMD_INVALID_ACC", "Compressed accelerometer delta header is truncated.");
      }
      const deltaWidth = payload[offset];
      const sampleCount = payload[offset + 1];
      offset += 2;
      if (deltaWidth < 1 || deltaWidth > 16) {
        throw new WebBluetoothError("PMD_INVALID_ACC", `Unsupported accelerometer delta width: ${deltaWidth}.`);
      }
      const bitLength = sampleCount * deltaWidth * 3;
      const byteLength = Math.ceil(bitLength / 8);
      if (offset + byteLength > payload.length) {
        throw new WebBluetoothError("PMD_INVALID_ACC", "Compressed accelerometer delta block is truncated.");
      }
      const deltaBytes = payload.subarray(offset, offset + byteLength);
      let bitOffset = 0;
      for (let index = 0; index < sampleCount; index += 1) {
        xMg = Math.max(-32768, Math.min(32767, xMg + readSignedBits(deltaBytes, bitOffset, deltaWidth)));
        bitOffset += deltaWidth;
        yMg = Math.max(-32768, Math.min(32767, yMg + readSignedBits(deltaBytes, bitOffset, deltaWidth)));
        bitOffset += deltaWidth;
        zMg = Math.max(-32768, Math.min(32767, zMg + readSignedBits(deltaBytes, bitOffset, deltaWidth)));
        bitOffset += deltaWidth;
        samples.push({ xMg, yMg, zMg });
      }
      offset += byteLength;
    }
    return samples;
  }

  function decodeHeartRate(value) {
    const bytes = asBytes(value);
    if (bytes.length < 2) return { beatsPerMinute: 0, rrIntervalsMs: [] };
    const flags = bytes[0];
    const uses16BitHeartRate = Boolean(flags & 0x01);
    let cursor = uses16BitHeartRate ? 3 : 2;
    const beatsPerMinute = uses16BitHeartRate && bytes.length >= 3
      ? bytes[1] | (bytes[2] << 8)
      : bytes[1];
    if (flags & 0x08) cursor += 2;
    const rrIntervalsMs = [];
    if (flags & 0x10) {
      while (cursor + 1 < bytes.length) {
        rrIntervalsMs.push((bytes[cursor] | (bytes[cursor + 1] << 8)) * (1000 / 1024));
        cursor += 2;
      }
    }
    return { beatsPerMinute, rrIntervalsMs };
  }

  function decodePmd(value) {
    const bytes = asBytes(value);
    if (bytes.length < 10) {
      throw new WebBluetoothError("PMD_FRAME_TOO_SHORT", "PMD frame is shorter than its 10-byte header.");
    }
    const measurement = bytes[0];
    const sensorTimestampNs = readUnsigned64Le(bytes, 1).toString();
    const frameType = bytes[9];
    const payload = bytes.subarray(10);

    if (measurement === MEASUREMENTS.ecg && frameType === 0x00) {
      if (payload.length % 3 !== 0) {
        throw new WebBluetoothError("PMD_INVALID_ECG", "ECG payload is not a sequence of signed 24-bit samples.");
      }
      const microvolts = [];
      for (let offset = 0; offset < payload.length; offset += 3) {
        microvolts.push(readSigned24Le(payload, offset));
      }
      return { kind: "ecg", sensorTimestampNs, microvolts };
    }

    if (measurement !== MEASUREMENTS.accelerometer) {
      throw new WebBluetoothError("PMD_UNSUPPORTED_FRAME", "Unsupported PMD measurement or frame type.");
    }
    const compressed = Boolean(frameType & 0x80);
    const baseFrameType = frameType & 0x7f;
    let samples = [];
    if (!compressed && baseFrameType === 0x01) {
      if (payload.length % 6 !== 0) {
        throw new WebBluetoothError("PMD_INVALID_ACC", "Accelerometer payload has an invalid length.");
      }
      for (let offset = 0; offset < payload.length; offset += 6) {
        samples.push({
          xMg: readSigned16Le(payload, offset),
          yMg: readSigned16Le(payload, offset + 2),
          zMg: readSigned16Le(payload, offset + 4),
        });
      }
    } else if (compressed && (baseFrameType === 0x00 || baseFrameType === 0x01)) {
      samples = decodeCompressedAccelerometer(payload);
    } else {
      throw new WebBluetoothError("PMD_UNSUPPORTED_FRAME", "Unsupported PMD accelerometer frame type.");
    }
    return { kind: "accelerometer", sensorTimestampNs, samples };
  }

  function defaultBreathingSettings() {
    return {
      axes: [true, false, true],
      calibrationWindowSeconds: 12,
      minimumAxisRangeG: 0.01,
      smoothingWindowSeconds: 0.75,
      sensitivity: 0.60,
      staleTimeoutSeconds: 3,
      invertDirection: false,
      adaptiveBounds: true,
      adaptiveWindowSeconds: 20,
      lowerQuantile: 0.05,
      upperQuantile: 0.95,
    };
  }

  function finiteClamped(value, fallback, low, high) {
    const number = Number(value);
    return Math.max(low, Math.min(high, Number.isFinite(number) ? number : fallback));
  }

  function clampBreathingSettings(value = {}) {
    const settings = { ...defaultBreathingSettings(), ...value };
    const axes = Array.isArray(settings.axes) ? settings.axes.slice(0, 3).map(Boolean) : [true, false, true];
    while (axes.length < 3) axes.push(false);
    settings.axes = axes.filter(Boolean).length >= 2 ? axes : [true, false, true];
    settings.calibrationWindowSeconds = finiteClamped(settings.calibrationWindowSeconds, 12, 1, 60);
    settings.minimumAxisRangeG = finiteClamped(settings.minimumAxisRangeG, 0.01, 0.001, 0.25);
    settings.smoothingWindowSeconds = finiteClamped(settings.smoothingWindowSeconds, 0.75, 0.05, 5);
    settings.sensitivity = finiteClamped(settings.sensitivity, 0.60, 0, 1);
    settings.staleTimeoutSeconds = finiteClamped(settings.staleTimeoutSeconds, 3, 0.25, 30);
    settings.adaptiveWindowSeconds = finiteClamped(settings.adaptiveWindowSeconds, 20, 5, 300);
    settings.lowerQuantile = finiteClamped(settings.lowerQuantile, 0.05, 0, 0.40);
    settings.upperQuantile = finiteClamped(settings.upperQuantile, 0.95, 0.60, 1);
    if (settings.upperQuantile - settings.lowerQuantile < 0.10) {
      settings.lowerQuantile = 0.05;
      settings.upperQuantile = 0.95;
    }
    settings.invertDirection = Boolean(settings.invertDirection);
    settings.adaptiveBounds = settings.adaptiveBounds !== false;
    return settings;
  }

  function dot(left, right) {
    return left[0] * right[0] + left[1] * right[1] + left[2] * right[2];
  }

  function subtract(left, right) {
    return [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
  }

  function quantile(sorted, fraction) {
    const position = (sorted.length - 1) * fraction;
    const low = Math.floor(position);
    const high = Math.ceil(position);
    return sorted[low] + (sorted[high] - sorted[low]) * (position - low);
  }

  function inverseLerp(low, high, value) {
    if (Math.abs(high - low) < 1e-8) return 0.5;
    return Math.max(0, Math.min(1, (value - low) / (high - low)));
  }

  class BreathingProcessor {
    constructor(settings = {}) {
      this.reset(settings);
    }

    reset(settings = this.settings) {
      this.settings = clampBreathingSettings(settings);
      this.filtered = [0, 0, 0];
      this.hasFiltered = false;
      this.calibration = [];
      this.lastCalibrationAttemptSeconds = Number.NEGATIVE_INFINITY;
      this.center = [0, 0, 0];
      this.axis = [0, 1, 0];
      this.boundMin = -0.02;
      this.boundMax = 0.02;
      this.calibrationSpan = 0.04;
      this.calibrated = false;
      this.projection = 0;
      this.adaptiveProjections = [];
      this.lastAdaptiveUpdateSeconds = 0;
      this.lastEmittedVolume = 0.5;
      this.hasEmittedVolume = false;
      this.elapsedSeconds = 0;
      this.lastPushAt = null;
    }

    applySettings(settings) {
      const next = clampBreathingSettings(settings);
      if (JSON.stringify(next) !== JSON.stringify(this.settings)) this.reset(next);
    }

    calibrationTarget() {
      return Math.round(this.settings.calibrationWindowSeconds * SAMPLE_RATE_HZ);
    }

    smoothingAlpha() {
      return Math.max(0.001, Math.min(1, 2 / (this.settings.smoothingWindowSeconds * SAMPLE_RATE_HZ + 1)));
    }

    phaseDeltaThreshold() {
      return 0.0005 + ((1 - this.settings.sensitivity) ** 2) * 0.015625;
    }

    push(samples, nowMilliseconds = performance.now()) {
      if (!Array.isArray(samples) || !samples.length) return null;
      const stale = this.lastPushAt !== null
        && nowMilliseconds - this.lastPushAt > this.settings.staleTimeoutSeconds * 1000;
      this.lastPushAt = nowMilliseconds;
      let latestVolume = this.lastEmittedVolume;
      const alpha = this.smoothingAlpha();

      for (const sample of samples) {
        const current = [sample.xMg / 1000, sample.yMg / 1000, sample.zMg / 1000]
          .map((axis, index) => this.settings.axes[index] ? axis : 0);
        if (!this.hasFiltered) {
          this.filtered = current;
          this.hasFiltered = true;
        } else {
          this.filtered = this.filtered.map((axis, index) => axis + (current[index] - axis) * alpha);
        }
        this.elapsedSeconds += 1 / SAMPLE_RATE_HZ;
        if (!this.calibrated) {
          this.calibration.push([...this.filtered]);
          const target = this.calibrationTarget();
          if (this.calibration.length > target) this.calibration.splice(0, this.calibration.length - target);
          if (this.calibration.length === target
            && this.elapsedSeconds - this.lastCalibrationAttemptSeconds >= 0.5) {
            this.lastCalibrationAttemptSeconds = this.elapsedSeconds;
            this.tryCalibrate();
          }
        }
        if (this.calibrated) {
          this.projection = dot(subtract(this.filtered, this.center), this.axis);
          this.updateAdaptiveBounds();
          latestVolume = inverseLerp(this.boundMin, this.boundMax, this.projection);
        }
      }

      let phase = 0;
      if (this.calibrated && !stale) {
        if (!this.hasEmittedVolume) {
          this.hasEmittedVolume = true;
        } else {
          const delta = latestVolume - this.lastEmittedVolume;
          if (delta > this.phaseDeltaThreshold()) phase = 1;
          else if (delta < -this.phaseDeltaThreshold()) phase = -1;
        }
      }
      this.lastEmittedVolume = latestVolume;
      const values = [
        { id: "breathing_calibration", value: Math.max(0, Math.min(1, this.calibration.length / this.calibrationTarget())) },
        { id: "breathing_phase", value: phase },
      ];
      if (this.calibrated) {
        values.push(
          { id: "acc_breathing_magnitude", value: this.projection },
          { id: "breathing_volume", value: latestVolume },
          { id: "breathing_axis_range", value: this.boundMax - this.boundMin },
        );
      }
      return {
        calibrated: this.calibrated,
        phase,
        magnitudeG: this.projection,
        volume01: latestVolume,
        axisRangeG: this.boundMax - this.boundMin,
        timeSeconds: this.elapsedSeconds,
        values,
      };
    }

    tryCalibrate() {
      const count = this.calibration.length;
      const center = [0, 0, 0];
      for (const sample of this.calibration) {
        for (let index = 0; index < 3; index += 1) center[index] += sample[index] / count;
      }
      const covariance = Array.from({ length: 3 }, () => [0, 0, 0]);
      for (const sample of this.calibration) {
        const delta = subtract(sample, center);
        for (let row = 0; row < 3; row += 1) {
          for (let column = 0; column < 3; column += 1) {
            covariance[row][column] += delta[row] * delta[column] / count;
          }
        }
      }
      let dominantDimension = 0;
      for (let index = 1; index < 3; index += 1) {
        if (covariance[index][index] > covariance[dominantDimension][dominantDimension]) dominantDimension = index;
      }
      let axis = [0, 0, 0];
      axis[dominantDimension] = 1;
      for (let iteration = 0; iteration < 8; iteration += 1) {
        const next = covariance.map((row) => dot(row, axis));
        const magnitude = Math.sqrt(dot(next, next));
        if (magnitude < 1e-8) return;
        axis = next.map((value) => value / magnitude);
      }
      if (this.settings.invertDirection) axis = axis.map((value) => -value);
      const projections = this.calibration
        .map((sample) => dot(subtract(sample, center), axis))
        .sort((left, right) => left - right);
      let low = quantile(projections, this.settings.lowerQuantile);
      let high = quantile(projections, this.settings.upperQuantile);
      const rawRange = high - low;
      if (rawRange < this.settings.minimumAxisRangeG) return;
      const ease = rawRange * 0.03;
      low += ease;
      high -= ease;
      if (high <= low) return;
      this.center = center;
      this.axis = axis;
      this.boundMin = low;
      this.boundMax = high;
      this.calibrationSpan = high - low;
      this.calibrated = true;
      this.adaptiveProjections = [];
    }

    updateAdaptiveBounds() {
      if (!this.settings.adaptiveBounds) return;
      const last = this.adaptiveProjections.at(-1);
      if (last && this.elapsedSeconds - last.time < 0.05) return;
      this.adaptiveProjections.push({ time: this.elapsedSeconds, value: this.projection });
      const cutoff = this.elapsedSeconds - this.settings.adaptiveWindowSeconds;
      while (this.adaptiveProjections.length && this.adaptiveProjections[0].time < cutoff) {
        this.adaptiveProjections.shift();
      }
      if (this.elapsedSeconds - this.lastAdaptiveUpdateSeconds < 0.5
        || this.adaptiveProjections.length < 80) return;
      this.lastAdaptiveUpdateSeconds = this.elapsedSeconds;
      const values = this.adaptiveProjections.map((entry) => entry.value).sort((left, right) => left - right);
      const low = quantile(values, this.settings.lowerQuantile);
      const high = quantile(values, this.settings.upperQuantile);
      const span = high - low;
      if (span < this.settings.minimumAxisRangeG
        || span < this.calibrationSpan * 0.5
        || span > this.calibrationSpan * 2) return;
      this.boundMin += (low - this.boundMin) * 0.2;
      this.boundMax += (high - this.boundMax) * 0.2;
    }
  }

  function supportStatus() {
    if (!window.isSecureContext) {
      return { supported: false, reason: "Web Bluetooth requires HTTPS or localhost." };
    }
    if (typeof navigator.bluetooth?.requestDevice !== "function") {
      return { supported: false, reason: "Use Chrome or Edge with Web Bluetooth support." };
    }
    return { supported: true, reason: "Chromium Web Bluetooth · experimental" };
  }

  function normalizeBrowserError(error) {
    if (error instanceof WebBluetoothError) return error;
    if (error?.name === "NotFoundError") {
      return new WebBluetoothError("BLUETOOTH_CHOOSER_CANCELLED", "No Polar H10 was selected.", true);
    }
    if (error?.name === "SecurityError" || error?.name === "NotAllowedError") {
      return new WebBluetoothError("BLUETOOTH_PERMISSION_DENIED", "Bluetooth permission was not granted.", true);
    }
    if (error?.name === "NetworkError") {
      return new WebBluetoothError("BLUETOOTH_CONNECTION_FAILED", "The H10 Bluetooth connection failed.", true);
    }
    return new WebBluetoothError(
      "BROWSER_BLE_FAILED",
      error?.message || "The browser could not connect to the Polar H10.",
      true,
    );
  }

  class PolarWebBluetoothSession {
    constructor() {
      this.device = null;
      this.server = null;
      this.control = null;
      this.pmdData = null;
      this.heartRate = null;
      this.onEvent = null;
      this.disconnecting = false;
      this.connected = false;
      this.wakeLock = null;
      this.wakeLockWanted = false;
      this.outputs = new Set(["raw_ecg", "raw_acc"]);
      this.breathing = new BreathingProcessor();
      this.boundPmd = (event) => this.handlePmd(event);
      this.boundHeartRate = (event) => this.handleHeartRate(event);
      this.boundControl = (event) => this.handleControl(event);
      this.boundDisconnected = () => this.handleDisconnected();
      this.boundVisibility = () => {
        if (this.wakeLockWanted && document.visibilityState === "visible") {
          void this.requestWakeLock();
        }
      };
      document.addEventListener("visibilitychange", this.boundVisibility);
    }

    updateConfig(config = {}) {
      this.outputs = new Set(config.outputs || []);
      const candidates = ["breathing_phase", "acc_breathing_magnitude"];
      let settings = defaultBreathingSettings();
      for (const id of candidates) {
        const processing = config.metricOptions?.[id]?.processing || {};
        if (processing.breathing || processing.breathingPhase) {
          settings = processing.breathing || processing.breathingPhase;
          break;
        }
      }
      this.breathing.applySettings(settings);
    }

    async connect(onEvent, config = {}) {
      const status = supportStatus();
      if (!status.supported) throw new WebBluetoothError("WEB_BLUETOOTH_UNAVAILABLE", status.reason);
      await this.disconnect({ emit: false });
      this.onEvent = onEvent;
      this.updateConfig(config);
      this.disconnecting = false;
      try {
        this.emit({ kind: "status", message: "Choose your Polar H10 in the browser Bluetooth prompt…" });
        this.device = await navigator.bluetooth.requestDevice({
          filters: [{ namePrefix: "Polar H10" }],
          optionalServices: [UUIDS.pmdService, UUIDS.heartRateService, UUIDS.batteryService],
        });
        this.device.addEventListener("gattserverdisconnected", this.boundDisconnected);
        this.emit({ kind: "status", message: "Connecting and discovering Polar PMD services…" });
        this.server = await this.device.gatt.connect();
        const pmdService = await this.server.getPrimaryService(UUIDS.pmdService);
        this.control = await pmdService.getCharacteristic(UUIDS.pmdControl);
        this.pmdData = await pmdService.getCharacteristic(UUIDS.pmdData);
        this.control.addEventListener("characteristicvaluechanged", this.boundControl);
        this.pmdData.addEventListener("characteristicvaluechanged", this.boundPmd);
        await this.pmdData.startNotifications();
        await this.control.startNotifications();

        try {
          const heartRateService = await this.server.getPrimaryService(UUIDS.heartRateService);
          this.heartRate = await heartRateService.getCharacteristic(UUIDS.heartRateMeasurement);
          this.heartRate.addEventListener("characteristicvaluechanged", this.boundHeartRate);
          await this.heartRate.startNotifications();
        } catch {
          this.heartRate = null;
        }

        await this.writeControl(COMMANDS.startEcg);
        await this.writeControl(COMMANDS.startAccelerometer);
        let batteryPercent = null;
        try {
          const batteryService = await this.server.getPrimaryService(UUIDS.batteryService);
          const battery = await batteryService.getCharacteristic(UUIDS.batteryLevel);
          const value = asBytes(await battery.readValue());
          batteryPercent = value.length ? value[0] : null;
        } catch {
          batteryPercent = null;
        }
        this.connected = true;
        this.wakeLockWanted = true;
        const screenAwake = await this.requestWakeLock();
        this.emit({
          kind: "connection",
          connected: true,
          streaming: true,
          simulated: false,
          transport: "web-bluetooth",
          deviceName: this.device.name || "Polar H10",
          batteryPercent,
          message: screenAwake
            ? "Experimental browser BLE · foreground screen wake lock active"
            : "Experimental browser BLE · keep this tab visible and the screen awake",
        });
      } catch (error) {
        await this.disconnect({ emit: false });
        this.onEvent = null;
        throw normalizeBrowserError(error);
      }
    }

    async writeControl(command) {
      const value = Uint8Array.from(command);
      if (typeof this.control.writeValueWithResponse === "function") {
        await this.control.writeValueWithResponse(value);
      } else {
        await this.control.writeValue(value);
      }
    }

    async requestWakeLock() {
      if (!this.wakeLockWanted || document.visibilityState !== "visible"
        || typeof navigator.wakeLock?.request !== "function") return false;
      if (this.wakeLock && !this.wakeLock.released) return true;
      try {
        const sentinel = await navigator.wakeLock.request("screen");
        this.wakeLock = sentinel;
        sentinel.addEventListener("release", () => {
          if (this.wakeLock === sentinel) this.wakeLock = null;
        }, { once: true });
        return true;
      } catch {
        this.wakeLock = null;
        return false;
      }
    }

    async releaseWakeLock() {
      this.wakeLockWanted = false;
      const sentinel = this.wakeLock;
      this.wakeLock = null;
      if (sentinel && !sentinel.released) {
        try { await sentinel.release(); } catch { /* best effort */ }
      }
    }

    handlePmd(event) {
      try {
        const frame = decodePmd(event.target.value);
        this.emit(frame);
        if (frame.kind === "accelerometer"
          && (this.outputs.has("breathing_phase") || this.outputs.has("acc_breathing_magnitude"))) {
          const snapshot = this.breathing.push(frame.samples);
          if (snapshot) this.emit({ kind: "metrics", values: snapshot.values });
        }
      } catch (error) {
        this.emit({ kind: "error", message: `Skipped malformed browser PMD frame: ${error.message}` });
      }
    }

    handleHeartRate(event) {
      const frame = decodeHeartRate(event.target.value);
      const values = [{ id: "heart_rate", value: frame.beatsPerMinute }];
      for (const interval of frame.rrIntervalsMs) values.push({ id: "rr_interval", value: interval });
      this.emit({ kind: "metrics", values });
    }

    handleControl(event) {
      const bytes = asBytes(event.target.value);
      if (bytes.length >= 4 && bytes[0] === 0xf0 && bytes[3] !== 0) {
        this.emit({
          kind: "error",
          message: `Polar PMD rejected command 0x${bytes[1].toString(16)} for measurement 0x${bytes[2].toString(16)} (status ${bytes[3]}).`,
        });
      }
    }

    async disconnect({ emit = true } = {}) {
      if (this.disconnecting) return { emitted: false };
      this.disconnecting = true;
      const wasConnected = this.connected;
      const deviceName = this.device?.name || "Polar H10";
      try {
        if (this.server?.connected && this.control) {
          try { await this.writeControl(COMMANDS.stopEcg); } catch { /* best effort */ }
          try { await this.writeControl(COMMANDS.stopAccelerometer); } catch { /* best effort */ }
        }
        await this.stopCharacteristic(this.pmdData, this.boundPmd);
        await this.stopCharacteristic(this.control, this.boundControl);
        await this.stopCharacteristic(this.heartRate, this.boundHeartRate);
        if (this.device) this.device.removeEventListener("gattserverdisconnected", this.boundDisconnected);
        if (this.server?.connected) this.server.disconnect();
      } finally {
        await this.releaseWakeLock();
        this.connected = false;
        this.server = null;
        this.control = null;
        this.pmdData = null;
        this.heartRate = null;
        this.device = null;
        this.disconnecting = false;
      }
      if (emit && (wasConnected || this.onEvent)) {
        this.emit({
          kind: "connection",
          connected: false,
          streaming: false,
          simulated: false,
          transport: "web-bluetooth",
          deviceName,
          batteryPercent: null,
          message: "Browser Bluetooth disconnected",
        });
      }
      if (emit) this.onEvent = null;
      return { emitted: emit && Boolean(wasConnected) };
    }

    async stopCharacteristic(characteristic, listener) {
      if (!characteristic) return;
      characteristic.removeEventListener("characteristicvaluechanged", listener);
      try { await characteristic.stopNotifications(); } catch { /* best effort */ }
    }

    handleDisconnected() {
      if (this.disconnecting) return;
      const deviceName = this.device?.name || "Polar H10";
      this.connected = false;
      void this.releaseWakeLock();
      this.server = null;
      this.control = null;
      this.pmdData = null;
      this.heartRate = null;
      this.device = null;
      this.emit({
        kind: "connection",
        connected: false,
        streaming: false,
        simulated: false,
        transport: "web-bluetooth",
        deviceName,
        batteryPercent: null,
        message: "The H10 left Bluetooth range or disconnected",
      });
      this.onEvent = null;
    }

    emit(event) {
      if (typeof this.onEvent === "function") this.onEvent(event);
    }
  }

  const session = new PolarWebBluetoothSession();
  const api = {
    UUIDS,
    COMMANDS,
    moduleId: "web-bluetooth-polar-h10",
    supportStatus,
    decodeHeartRate,
    decodePmd,
    createBreathingProcessor(settings) {
      return new BreathingProcessor(settings);
    },
    connect(onEvent, config) {
      return session.connect(onEvent, config);
    },
    disconnect() {
      return session.disconnect();
    },
    updateConfig(config) {
      session.updateConfig(config);
    },
  };

  window.PolarWebBluetooth = Object.freeze(api);
})();
