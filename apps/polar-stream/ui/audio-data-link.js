(() => {
  "use strict";

  const SCHEMA_VERSION = 1;
  const SYMBOL_RATE = 11_025;
  const FRAME_INTERVAL_MS = 125;
  const MAX_AUDIO_LEAD_SECONDS = 1.25;
  const MAX_PENDING_SAMPLES = 4_096;
  const AMPLITUDE = 0.22;
  const PREAMBLE = Uint8Array.from([0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd3, 0x91]);
  const MAGIC = Uint8Array.from([0x50, 0x53]); // "PS"
  const SECTION = Object.freeze({ ecg: 1, accelerometer: 2, metrics: 3 });
  const encoder = new TextEncoder();

  function crcTable() {
    const table = new Uint32Array(256);
    for (let index = 0; index < 256; index += 1) {
      let value = index;
      for (let bit = 0; bit < 8; bit += 1) {
        value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0);
      }
      table[index] = value >>> 0;
    }
    return table;
  }

  const CRC_TABLE = crcTable();

  function crc32(bytes) {
    let value = 0xffffffff;
    for (const byte of bytes) value = (value >>> 8) ^ CRC_TABLE[(value ^ byte) & 0xff];
    return (value ^ 0xffffffff) >>> 0;
  }

  function pushU16(target, value) {
    const number = Math.max(0, Math.min(0xffff, Math.floor(Number(value) || 0)));
    target.push(number & 0xff, (number >>> 8) & 0xff);
  }

  function pushU32(target, value) {
    const number = Number(value) >>> 0;
    target.push(number & 0xff, (number >>> 8) & 0xff, (number >>> 16) & 0xff, (number >>> 24) & 0xff);
  }

  function pushU64(target, value) {
    let number;
    try { number = BigInt(value || 0); } catch { number = 0n; }
    for (let index = 0; index < 8; index += 1) {
      target.push(Number(number & 0xffn));
      number >>= 8n;
    }
  }

  function pushInt24(target, value) {
    const number = Math.max(-8_388_608, Math.min(8_388_607, Math.round(Number(value) || 0)));
    const unsigned = number < 0 ? number + 0x1000000 : number;
    target.push(unsigned & 0xff, (unsigned >>> 8) & 0xff, (unsigned >>> 16) & 0xff);
  }

  function pushInt16(target, value) {
    const number = Math.max(-32_768, Math.min(32_767, Math.round(Number(value) || 0)));
    const unsigned = number < 0 ? number + 0x10000 : number;
    target.push(unsigned & 0xff, (unsigned >>> 8) & 0xff);
  }

  function pushFloat32(target, value) {
    const buffer = new ArrayBuffer(4);
    new DataView(buffer).setFloat32(0, Number(value) || 0, true);
    target.push(...new Uint8Array(buffer));
  }

  function readU16(bytes, offset) {
    return bytes[offset] | (bytes[offset + 1] << 8);
  }

  function readU32(bytes, offset) {
    return (
      bytes[offset]
      | (bytes[offset + 1] << 8)
      | (bytes[offset + 2] << 16)
      | (bytes[offset + 3] << 24)
    ) >>> 0;
  }

  function makeSection(type, count, timestamp, payload) {
    const section = [type];
    pushU16(section, count);
    pushU64(section, timestamp);
    pushU16(section, payload.length);
    section.push(...payload);
    return section;
  }

  function encodeBatch(batch, sequence = 0) {
    const sections = [];
    if (batch.ecg.length) {
      const payload = [];
      for (const value of batch.ecg) pushInt24(payload, value);
      sections.push(makeSection(SECTION.ecg, batch.ecg.length, batch.ecgTimestamp, payload));
    }
    if (batch.accelerometer.length) {
      const payload = [];
      for (const sample of batch.accelerometer) {
        pushInt16(payload, sample.xMg ?? sample.x_mg);
        pushInt16(payload, sample.yMg ?? sample.y_mg);
        pushInt16(payload, sample.zMg ?? sample.z_mg);
      }
      sections.push(makeSection(
        SECTION.accelerometer,
        batch.accelerometer.length,
        batch.accTimestamp,
        payload,
      ));
    }
    if (batch.metrics.length) {
      const payload = [];
      for (const metric of batch.metrics) {
        const id = encoder.encode(String(metric.id || "").slice(0, 64));
        payload.push(id.length, ...id);
        pushFloat32(payload, metric.value);
      }
      sections.push(makeSection(SECTION.metrics, batch.metrics.length, 0, payload));
    }
    if (!sections.length) return null;

    const payload = [sections.length];
    for (const section of sections) payload.push(...section);
    if (payload.length > 0xffff) throw new Error("Audio data frame exceeds its 65,535-byte payload limit.");
    const packet = [...MAGIC, (SCHEMA_VERSION << 4) | 1];
    pushU16(packet, sequence);
    pushU16(packet, payload.length);
    packet.push(...payload);
    pushU32(packet, crc32(packet));
    return Uint8Array.from(packet);
  }

  function decodePacket(packet) {
    const bytes = packet instanceof Uint8Array ? packet : Uint8Array.from(packet || []);
    if (bytes.length < 11 || bytes[0] !== MAGIC[0] || bytes[1] !== MAGIC[1]) {
      throw new Error("Not a Polar Stream audio packet.");
    }
    const payloadLength = readU16(bytes, 5);
    const expectedLength = 7 + payloadLength + 4;
    if (bytes.length !== expectedLength) throw new Error("Audio packet length does not match its header.");
    const expectedCrc = readU32(bytes, bytes.length - 4);
    const actualCrc = crc32(bytes.subarray(0, bytes.length - 4));
    if (actualCrc !== expectedCrc) throw new Error("Audio packet CRC32 check failed.");
    return Object.freeze({
      schemaVersion: bytes[2] >>> 4,
      type: bytes[2] & 0x0f,
      sequence: readU16(bytes, 3),
      payload: bytes.slice(7, bytes.length - 4),
    });
  }

  function packetWaveform(packet, sampleRate) {
    const bytes = new Uint8Array(PREAMBLE.length + packet.length);
    bytes.set(PREAMBLE);
    bytes.set(packet, PREAMBLE.length);
    const bitCount = bytes.length * 8;
    const symbolCount = Math.ceil(bitCount / 2);
    const leadingSamples = Math.ceil(sampleRate * 0.002);
    const trailingSamples = Math.ceil(sampleRate * 0.004);
    const dataSamples = Math.ceil((symbolCount / SYMBOL_RATE) * sampleRate);
    const left = new Float32Array(leadingSamples + dataSamples + trailingSamples);
    const right = new Float32Array(left.length);
    const bitAt = (index) => index < bitCount
      ? (bytes[Math.floor(index / 8)] >>> (7 - (index % 8))) & 1
      : 0;
    for (let sample = 0; sample < dataSamples; sample += 1) {
      const position = (sample * SYMBOL_RATE) / sampleRate;
      const symbol = Math.floor(position);
      const firstHalf = position - symbol < 0.5;
      const leftBit = bitAt(symbol * 2);
      const rightBit = bitAt(symbol * 2 + 1);
      const index = leadingSamples + sample;
      left[index] = (leftBit === Number(firstHalf) ? AMPLITUDE : -AMPLITUDE);
      right[index] = (rightBit === Number(firstHalf) ? AMPLITUDE : -AMPLITUDE);
    }
    return { left, right };
  }

  function emptyBatch() {
    return {
      ecg: [],
      ecgTimestamp: 0,
      accelerometer: [],
      accTimestamp: 0,
      metrics: [],
    };
  }

  class AudioDataLink {
    constructor() {
      this.listeners = new Set();
      this.context = null;
      this.gain = null;
      this.timer = 0;
      this.enabled = false;
      this.error = null;
      this.sequence = 0;
      this.frameCount = 0;
      this.nextStart = 0;
      this.sources = new Set();
      this.batch = emptyBatch();
      this.streamName = "Polar-H10";
    }

    configure(config = {}) {
      this.streamName = String(config.streamName || this.streamName);
    }

    supportStatus() {
      const AudioContextClass = window.AudioContext || window.webkitAudioContext;
      return AudioContextClass
        ? { supported: true, reason: "Stereo Manchester PCM · 22.05 kbit/s" }
        : { supported: false, reason: "This WebView does not provide Web Audio output." };
    }

    subscribe(listener) {
      this.listeners.add(listener);
      listener(this.snapshot());
      return () => this.listeners.delete(listener);
    }

    snapshot() {
      return Object.freeze({
        enabled: this.enabled,
        error: this.error,
        frameCount: this.frameCount,
        sampleRate: this.context?.sampleRate || null,
        bitRate: SYMBOL_RATE * 2,
        queuedSeconds: this.context ? Math.max(0, this.nextStart - this.context.currentTime) : 0,
      });
    }

    notify() {
      const status = this.snapshot();
      for (const listener of this.listeners) listener(status);
    }

    async enable(config = {}) {
      this.configure(config);
      if (this.enabled) return this.snapshot();
      const support = this.supportStatus();
      if (!support.supported) throw new Error(support.reason);
      const AudioContextClass = window.AudioContext || window.webkitAudioContext;
      this.context ||= new AudioContextClass({ latencyHint: "interactive" });
      await this.context.resume();
      if (this.context.sampleRate < 40_000) {
        throw new Error(`Audio sample rate ${this.context.sampleRate} Hz is too low for the H10 data link.`);
      }
      this.gain ||= this.context.createGain();
      this.gain.gain.value = 1;
      if (!this.gain.__polarConnected) {
        this.gain.connect(this.context.destination);
        this.gain.__polarConnected = true;
      }
      this.enabled = true;
      this.error = null;
      this.nextStart = this.context.currentTime + 0.03;
      this.batch = emptyBatch();
      this.timer = window.setInterval(() => this.flush(), FRAME_INTERVAL_MS);
      this.notify();
      return this.snapshot();
    }

    disable() {
      if (this.timer) window.clearInterval(this.timer);
      this.timer = 0;
      this.enabled = false;
      this.batch = emptyBatch();
      for (const source of this.sources) {
        try { source.stop(); } catch { /* already ended */ }
        source.disconnect();
      }
      this.sources.clear();
      if (this.context?.state === "running") void this.context.suspend();
      this.notify();
      return this.snapshot();
    }

    capture(event) {
      if (!this.enabled || !event || typeof event !== "object") return;
      if (event.kind === "ecg") {
        this.batch.ecg.push(...(event.microvolts || []));
        this.batch.ecgTimestamp = event.sensorTimestampNs || 0;
      } else if (event.kind === "accelerometer") {
        this.batch.accelerometer.push(...(event.samples || []));
        this.batch.accTimestamp = event.sensorTimestampNs || 0;
      } else if (event.kind === "metrics") {
        this.batch.metrics.push(...(event.values || []));
      } else if (event.kind === "connection" && event.connected === false) {
        this.flush();
      }
      if (this.batch.ecg.length > MAX_PENDING_SAMPLES
        || this.batch.accelerometer.length > MAX_PENDING_SAMPLES
        || this.batch.metrics.length > 512) {
        this.fail("Audio output stopped because its bounded input batch overflowed.");
      }
    }

    flush() {
      if (!this.enabled) return;
      const batch = this.batch;
      this.batch = emptyBatch();
      let packet;
      try {
        packet = encodeBatch(batch, this.sequence);
      } catch (error) {
        this.fail(error.message || String(error));
        return;
      }
      if (!packet) return;
      this.sequence = (this.sequence + 1) & 0xffff;
      const waveform = packetWaveform(packet, this.context.sampleRate);
      const start = Math.max(this.context.currentTime + 0.02, this.nextStart);
      const end = start + waveform.left.length / this.context.sampleRate;
      if (end - this.context.currentTime > MAX_AUDIO_LEAD_SECONDS) {
        this.fail("Audio output stopped because the PCM modem could not keep up with the incoming data rate.");
        return;
      }
      const buffer = this.context.createBuffer(2, waveform.left.length, this.context.sampleRate);
      buffer.copyToChannel(waveform.left, 0);
      buffer.copyToChannel(waveform.right, 1);
      const source = this.context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.gain);
      source.addEventListener("ended", () => {
        this.sources.delete(source);
        source.disconnect();
        this.notify();
      }, { once: true });
      this.sources.add(source);
      source.start(start);
      this.nextStart = end;
      this.frameCount += 1;
      this.notify();
    }

    fail(message) {
      this.error = String(message);
      this.disable();
      this.error = String(message);
      this.notify();
      window.dispatchEvent(new CustomEvent("polar-stream-audio-error", { detail: this.error }));
    }
  }

  const link = new AudioDataLink();
  window.PolarAudioDataLink = Object.freeze({
    schemaVersion: SCHEMA_VERSION,
    symbolRate: SYMBOL_RATE,
    supportStatus: () => link.supportStatus(),
    configure: (config) => link.configure(config),
    enable: (config) => link.enable(config),
    disable: () => link.disable(),
    capture: (event) => link.capture(event),
    subscribe: (listener) => link.subscribe(listener),
    status: () => link.snapshot(),
    encodeBatch,
    decodePacket,
    packetWaveform,
  });
})();
