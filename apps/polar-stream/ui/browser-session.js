(() => {
  "use strict";

  const SCHEMA_VERSION = 2;
  const RECORDING_SCHEMA_VERSION = 3;
  const DEFAULT_MAX_ROWS = 300_000;
  const CHANNEL_NAME = "polar-stream-live-v1";
  const CSV_COLUMNS = [
    "host_timestamp_ms",
    "relative_time_s",
    "sensor_timestamp_ns",
    "source_id",
    "source_palette_id",
    "stream",
    "sample_index",
    "x_mg",
    "y_mg",
    "z_mg",
    "value",
    "unit",
  ];
  const units = Object.freeze({
    raw_ecg: "uV",
    raw_acc: "mg",
    raw_force: "N",
    acc_magnitude: "g",
    acc_breathing_magnitude: "g",
    breathing_volume: "0-1",
    breathing_phase: "class",
    breathing_calibration: "0-1",
    breathing_axis_range: "g",
    breathing_signal_confidence: "0-1",
    breathing_signal_ready: "0/1",
    heart_rate: "bpm",
    rr_interval: "ms",
  });

  function finite(value) {
    const number = Number(value);
    return Number.isFinite(number) ? number : null;
  }

  function csvCell(value) {
    if (value == null) return "";
    const text = String(value);
    return /[",\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
  }

  function csvRow(values) {
    return `${values.map(csvCell).join(",")}\n`;
  }

  function safeName(value) {
    const normalized = String(value || "Polar-H10")
      .trim()
      .replace(/[^A-Za-z0-9_-]+/g, "_")
      .replace(/^_+|_+$/g, "")
      .slice(0, 64);
    return normalized || "Polar-H10";
  }

  function timestampForFilename(date) {
    return date.toISOString().replace(/:/g, "-").replace(/\.\d{3}Z$/, "Z");
  }

  function sensorTimestamp(sensorTimestampNs, sampleIndex, sampleCount, rateHz) {
    if (sensorTimestampNs == null || sensorTimestampNs === 0 || sensorTimestampNs === "0") return "";
    try {
      const finalTimestamp = BigInt(sensorTimestampNs);
      const offset = BigInt(Math.round(((sampleCount - 1 - sampleIndex) * 1_000_000_000) / rateHz));
      return String(finalTimestamp - offset);
    } catch {
      return String(sensorTimestampNs);
    }
  }

  class SessionRecorder {
    constructor({ maxRows = DEFAULT_MAX_ROWS, now = () => Date.now() } = {}) {
      this.maxRows = Math.max(1, Math.floor(maxRows));
      this.now = now;
      this.listeners = new Set();
      this.config = {
        streamName: "Polar-H10",
        outputs: ["raw_ecg", "raw_acc"],
        metricOptions: {},
        metricUnits: {},
      };
      this.reset();
    }

    reset() {
      this.state = "idle";
      this.stopReason = null;
      this.startedAtMs = null;
      this.stoppedAtMs = null;
      this.source = null;
      this.sourcePalettes = new Map();
      this.rowCount = 0;
      this.chunks = [];
      this.pendingLines = [];
      this.notify();
    }

    configure(config = {}) {
      this.config = {
        streamName: safeName(config.streamName || this.config.streamName),
        outputs: Array.isArray(config.outputs) ? [...new Set(config.outputs)] : [...this.config.outputs],
        metricOptions: structuredClone(config.metricOptions || {}),
        metricUnits: structuredClone(config.metricUnits || this.config.metricUnits || {}),
      };
      this.notify();
    }

    subscribe(listener) {
      this.listeners.add(listener);
      listener(this.snapshot());
      return () => this.listeners.delete(listener);
    }

    snapshot() {
      return Object.freeze({
        state: this.state,
        stopReason: this.stopReason,
        rowCount: this.rowCount,
        maxRows: this.maxRows,
        startedAtMs: this.startedAtMs,
        stoppedAtMs: this.stoppedAtMs,
        hasData: this.rowCount > 0,
        streamName: this.config.streamName,
      });
    }

    notify() {
      const snapshot = this.snapshot();
      for (const listener of this.listeners) listener(snapshot);
    }

    start({ deviceName = "Browser input", inputKind = "browser", source = null } = {}) {
      if (this.state === "recording") return this.snapshot();
      if (this.rowCount > 0) {
        throw new Error("Export or discard the previous browser recording before starting another.");
      }
      this.state = "recording";
      this.stopReason = null;
      this.startedAtMs = this.now();
      this.stoppedAtMs = null;
      this.source = {
        deviceName: String(deviceName),
        inputKind: String(inputKind),
        id: String(source?.id || "browser-source"),
        palette: source?.palette || null,
      };
      this.rememberSource(this.source);
      this.notify();
      return this.snapshot();
    }

    stop(reason = "user") {
      if (this.state !== "recording") return this.snapshot();
      this.state = "stopped";
      this.stopReason = reason;
      this.stoppedAtMs = this.now();
      this.flushPending();
      this.notify();
      return this.snapshot();
    }

    discard() {
      this.reset();
    }

    flushPending() {
      if (!this.pendingLines.length) return;
      this.chunks.push(this.pendingLines.join(""));
      this.pendingLines = [];
    }

    append(values) {
      if (this.state !== "recording") return false;
      if (this.rowCount >= this.maxRows) {
        this.stop("capacity");
        return false;
      }
      this.pendingLines.push(csvRow(values));
      this.rowCount += 1;
      if (this.pendingLines.length >= 1024) this.flushPending();
      if (this.rowCount >= this.maxRows) this.stop("capacity");
      return true;
    }

    rememberSource(source) {
      const id = String(source?.id || this.source?.id || "browser-source");
      const palette = source?.palette || this.source?.palette;
      if (!palette?.id) return { id, paletteId: "" };
      this.sourcePalettes.set(id, structuredClone(palette));
      return { id, paletteId: String(palette.id) };
    }

    capture(event, hostTimestampMs = this.now()) {
      if (this.state !== "recording" || !event || typeof event !== "object") return;
      const elapsed = Math.max(0, (hostTimestampMs - this.startedAtMs) / 1000);
      const source = this.rememberSource(event.source || this.source);
      if (event.kind === "ecg") {
        const values = Array.isArray(event.microvolts) ? event.microvolts : [];
        for (let index = 0; index < values.length; index += 1) {
          const sampleHost = hostTimestampMs - ((values.length - 1 - index) * 1000) / 130;
          if (!this.append([
            sampleHost.toFixed(3),
            Math.max(0, elapsed - (values.length - 1 - index) / 130).toFixed(6),
            sensorTimestamp(event.sensorTimestampNs, index, values.length, 130), source.id, source.paletteId,
            "raw_ecg", index, "", "", "", finite(values[index]), units.raw_ecg,
          ])) break;
        }
        return;
      }
      if (event.kind === "accelerometer") {
        const samples = Array.isArray(event.samples) ? event.samples : [];
        for (let index = 0; index < samples.length; index += 1) {
          const sample = samples[index] || {};
          const x = finite(sample.xMg ?? sample.x_mg);
          const y = finite(sample.yMg ?? sample.y_mg);
          const z = finite(sample.zMg ?? sample.z_mg);
          const sampleHost = hostTimestampMs - ((samples.length - 1 - index) * 1000) / 200;
          const relative = Math.max(0, elapsed - (samples.length - 1 - index) / 200).toFixed(6);
          const deviceTime = sensorTimestamp(event.sensorTimestampNs, index, samples.length, 200);
          if (!this.append([
            sampleHost.toFixed(3), relative, deviceTime, source.id, source.paletteId, "raw_acc", index,
            x, y, z, "", units.raw_acc,
          ])) break;
        }
        return;
      }
      if (event.kind === "force") {
        const values = Array.isArray(event.values) ? event.values : [];
        const rateHz = 1_000_000 / Math.max(1, Number(event.samplePeriodUs) || 100_000);
        const stream = event.source?.slot ? `${event.source.slot}_raw_force` : "raw_force";
        for (let index = 0; index < values.length; index += 1) {
          const offsetSeconds = (values.length - 1 - index) / rateHz;
          if (!this.append([
            (hostTimestampMs - offsetSeconds * 1000).toFixed(3),
            Math.max(0, elapsed - offsetSeconds).toFixed(6),
            sensorTimestamp(event.hostReceiveTimestampNs, index, values.length, rateHz), source.id, source.paletteId,
            stream, index, "", "", "", finite(values[index]), units.raw_force,
          ])) break;
        }
        return;
      }
      if (event.kind === "metrics" && Array.isArray(event.values)) {
        for (let index = 0; index < event.values.length; index += 1) {
          const metric = event.values[index];
          if (!metric) continue;
          if (!this.append([
            hostTimestampMs.toFixed(3), elapsed.toFixed(6),
            sensorTimestamp(event.sensorTimestampNs, 0, 1, 1), source.id, source.paletteId, metric.id, index,
            "", "", "", finite(metric.value), this.config.metricUnits[metric.id] || units[metric.id] || "",
          ])) break;
        }
      }
    }

    header() {
      const started = new Date(this.startedAtMs || this.now()).toISOString();
      const stopped = this.stoppedAtMs ? new Date(this.stoppedAtMs).toISOString() : "";
      const paletteHeaders = [...this.sourcePalettes.entries()].map(([sourceId, palette]) => (
        `# source_palette,${csvCell(sourceId)},${csvCell(palette.id)},${palette.light.primary},${palette.light.secondary},${palette.dark.primary},${palette.dark.secondary}\n`
      ));
      return [
        "# Polar Stream browser recording\n",
        `# schema_version,${RECORDING_SCHEMA_VERSION}\n`,
        `# started_at_utc,${csvCell(started)}\n`,
        `# stopped_at_utc,${csvCell(stopped)}\n`,
        `# source,${csvCell(this.source?.deviceName || "Browser input")}\n`,
        `# input_kind,${csvCell(this.source?.inputKind || "browser")}\n`,
        "# source_palette_columns,source_id,palette_id,light_primary,light_secondary,dark_primary,dark_secondary\n",
        ...paletteHeaders,
        `# configured_outputs,${csvCell(this.config.outputs.join("|"))}\n`,
        `# stop_reason,${csvCell(this.stopReason || "export")}\n`,
        "# scope,All received raw ECG, ACC, and Go Direct force plus every metric event produced in this browser session.\n",
        "# timing,Sensor timestamps are reconstructed backwards from the final PMD frame timestamp when available.\n",
        csvRow(CSV_COLUMNS),
      ];
    }

    createBlob() {
      if (!this.rowCount) throw new Error("There is no browser recording to export.");
      if (this.state === "recording") this.stop("export");
      this.flushPending();
      return new Blob([...this.header(), ...this.chunks], { type: "text/csv;charset=utf-8" });
    }

    download() {
      const blob = this.createBlob();
      const started = new Date(this.startedAtMs || this.now());
      const filename = `${safeName(this.config.streamName)}_${timestampForFilename(started)}.csv`;
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = filename;
      anchor.hidden = true;
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
      return filename;
    }
  }

  const recorder = new SessionRecorder();
  let channel = null;

  function publish(event) {
    const hostTimestampMs = Date.now();
    recorder.capture(event, hostTimestampMs);
    if (event?.kind === "connection" && event.connected === false && recorder.snapshot().state === "recording") {
      recorder.stop("input-disconnected");
    }
    const detail = { schemaVersion: SCHEMA_VERSION, hostTimestampMs, event };
    window.dispatchEvent(new CustomEvent("polar-stream-data", { detail }));
    if (typeof BroadcastChannel === "function") {
      try {
        channel ||= new BroadcastChannel(CHANNEL_NAME);
        channel.postMessage(detail);
      } catch {
        // CustomEvent remains available if BroadcastChannel is blocked.
      }
    }
  }

  window.PolarBrowserSession = Object.freeze({
    schemaVersion: SCHEMA_VERSION,
    channelName: CHANNEL_NAME,
    configure: (config) => recorder.configure(config),
    start: (context) => recorder.start(context),
    stop: (reason) => recorder.stop(reason),
    discard: () => recorder.discard(),
    download: () => recorder.download(),
    publish,
    subscribe: (listener) => recorder.subscribe(listener),
    status: () => recorder.snapshot(),
    createRecorder: (options) => new SessionRecorder(options),
  });
})();
