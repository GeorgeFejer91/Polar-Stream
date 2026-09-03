(() => {
  "use strict";

  const SCHEMA_VERSION = 2;
  const RECORDING_SCHEMA_VERSION = 3;
  const DEFAULT_MAX_ROWS = 300_000;
  const MAX_SOURCE_IDENTITIES = 32;
  const MAX_SOURCE_FIELD_LENGTH = 128;
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
    vernier_breathing: "0-1",
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

  function boundedSourceField(value, fallback = "") {
    const text = String(value ?? fallback)
      .replace(/[\u0000-\u001f\u007f]/g, " ")
      .trim()
      .slice(0, MAX_SOURCE_FIELD_LENGTH);
    if (!text) return String(fallback).slice(0, MAX_SOURCE_FIELD_LENGTH);
    return /^[=+\-@]/.test(text) ? `'${text}` : text;
  }

  function inferredInputKind(eventKind) {
    if (eventKind === "force") return "vernierGoDirect";
    if (eventKind === "ecg" || eventKind === "accelerometer") return "polarH10";
    return "";
  }

  function inferredDeviceFamily(source, inputKind, eventKind) {
    const explicit = String(source?.deviceFamily || "").trim().toLowerCase();
    if (explicit === "polar" || explicit === "vernier") return explicit;
    const evidence = [inputKind, source?.deviceModel, source?.deviceName, source?.label]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    if (eventKind === "force" || /vernier|go\s*direct|gdx/.test(evidence)) return "vernier";
    if (eventKind === "ecg" || eventKind === "accelerometer" || /polar|h10/.test(evidence)) return "polar";
    return "unknown";
  }

  function boundedPalette(palette) {
    if (!palette?.id || !palette?.light || !palette?.dark) return null;
    return {
      id: boundedSourceField(palette.id),
      light: {
        primary: boundedSourceField(palette.light.primary),
        secondary: boundedSourceField(palette.light.secondary),
      },
      dark: {
        primary: boundedSourceField(palette.dark.primary),
        secondary: boundedSourceField(palette.dark.secondary),
      },
    };
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
      this.sessionDeviceName = null;
      this.sessionInputKind = null;
      this.source = null;
      this.sourceIdentities = new Map();
      this.sourcePalettes = new Map();
      this.activeSourceIds = new Set();
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

    start({ deviceName = "Browser input", inputKind = "browser", source = null, sources = [] } = {}) {
      if (this.state === "recording") return this.snapshot();
      if (this.rowCount > 0) {
        throw new Error("Export or discard the previous browser recording before starting another.");
      }
      this.state = "recording";
      this.stopReason = null;
      this.startedAtMs = this.now();
      this.stoppedAtMs = null;
      this.sessionDeviceName = boundedSourceField(deviceName, "Browser input");
      this.sessionInputKind = boundedSourceField(inputKind, "browser");
      this.source = {
        deviceName: boundedSourceField(source?.deviceName || deviceName, "Browser input"),
        inputKind: boundedSourceField(source?.inputKind || inputKind, "browser"),
        id: String(source?.id || "browser-source"),
        slot: String(source?.slot || source?.id || "browser-source"),
        label: source?.label ? String(source.label) : "",
        deviceFamily: source?.deviceFamily ? String(source.deviceFamily) : "",
        deviceModel: source?.deviceModel ? String(source.deviceModel) : "",
        palette: source?.palette || null,
      };
      const primary = this.rememberSource(this.source);
      if (primary) this.activeSourceIds.add(primary.id);
      for (const activeSource of [source, ...sources].filter(Boolean)) {
        const remembered = this.rememberSource(activeSource);
        if (remembered) this.activeSourceIds.add(remembered.id);
      }
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

    rememberSource(source, eventKind = "") {
      const candidate = source || this.source || {};
      const id = boundedSourceField(candidate.id || this.source?.id, "browser-source");
      const existing = this.sourceIdentities.get(id);
      if (!existing && this.sourceIdentities.size >= MAX_SOURCE_IDENTITIES) {
        this.stop("source-capacity");
        return null;
      }
      const isPrimary = id === boundedSourceField(this.source?.id, "browser-source");
      const inputKind = boundedSourceField(
        candidate.inputKind || existing?.inputKind || (isPrimary ? this.source?.inputKind : "") || inferredInputKind(eventKind),
        "unknown",
      );
      const inferredFamily = inferredDeviceFamily(candidate, inputKind, eventKind);
      const deviceFamily = boundedSourceField(
        inferredFamily !== "unknown" ? inferredFamily : existing?.deviceFamily,
        "unknown",
      );
      const palette = boundedPalette(candidate.palette)
        || existing?.palette
        || boundedPalette(isPrimary ? this.source?.palette : null);
      const identity = {
        id,
        slot: boundedSourceField(candidate.slot || existing?.slot || id, id),
        inputKind,
        deviceFamily,
        deviceName: boundedSourceField(
          candidate.deviceName || candidate.label || existing?.deviceName || (isPrimary ? this.source?.deviceName : ""),
          "unknown",
        ),
        paletteId: boundedSourceField(palette?.id || existing?.paletteId, ""),
        palette,
      };
      this.sourceIdentities.set(id, identity);
      if (palette?.id) this.sourcePalettes.set(id, palette);
      return identity;
    }

    capture(event, hostTimestampMs = this.now()) {
      if (this.state !== "recording" || !event || typeof event !== "object") return;
      const elapsed = Math.max(0, (hostTimestampMs - this.startedAtMs) / 1000);
      const source = this.rememberSource(event.source || this.source, event.kind);
      if (!source) return;
      if (event.kind === "connection") {
        if (event.connected === false) {
          this.activeSourceIds.delete(source.id);
          if (!this.activeSourceIds.size) this.stop("input-disconnected");
        } else if (event.connected === true) {
          this.activeSourceIds.add(source.id);
        }
        return;
      }
      this.activeSourceIds.add(source.id);
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
        const breathingValues = Array.isArray(event.breathingValues) ? event.breathingValues : [];
        const rateHz = 1_000_000 / Math.max(1, Number(event.samplePeriodUs) || 100_000);
        const stream = event.source?.slot ? `${source.slot}_raw_force` : "raw_force";
        const breathingStream = event.source?.slot ? `${source.slot}_vernier_breathing` : "vernier_breathing";
        for (let index = 0; index < values.length; index += 1) {
          const offsetSeconds = (values.length - 1 - index) / rateHz;
          if (!this.append([
            (hostTimestampMs - offsetSeconds * 1000).toFixed(3),
            Math.max(0, elapsed - offsetSeconds).toFixed(6),
            sensorTimestamp(event.hostReceiveTimestampNs, index, values.length, rateHz), source.id, source.paletteId,
            stream, index, "", "", "", finite(values[index]), units.raw_force,
          ])) break;
        }
        for (let index = 0; index < breathingValues.length; index += 1) {
          const offsetSeconds = (breathingValues.length - 1 - index) / rateHz;
          if (!this.append([
            (hostTimestampMs - offsetSeconds * 1000).toFixed(3),
            Math.max(0, elapsed - offsetSeconds).toFixed(6),
            sensorTimestamp(event.hostReceiveTimestampNs, index, breathingValues.length, rateHz), source.id, source.paletteId,
            breathingStream, index, "", "", "", finite(breathingValues[index]), units.vernier_breathing,
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
      const sourceIdentityHeaders = [...this.sourceIdentities.values()].map((source) => (
        `# source_identity,${csvCell(source.id)},${csvCell(source.slot)},${csvCell(source.inputKind)},${csvCell(source.deviceFamily)},${csvCell(source.deviceName)},${csvCell(source.paletteId)}\n`
      ));
      const paletteHeaders = [...this.sourcePalettes.entries()].map(([sourceId, palette]) => (
        `# source_palette,${csvCell(sourceId)},${csvCell(palette.id)},${csvCell(palette.light.primary)},${csvCell(palette.light.secondary)},${csvCell(palette.dark.primary)},${csvCell(palette.dark.secondary)}\n`
      ));
      return [
        "# Polar Stream browser recording\n",
        `# schema_version,${RECORDING_SCHEMA_VERSION}\n`,
        `# started_at_utc,${csvCell(started)}\n`,
        `# stopped_at_utc,${csvCell(stopped)}\n`,
        `# source,${csvCell(this.sessionDeviceName || "Browser input")}\n`,
        `# input_kind,${csvCell(this.sessionInputKind || "browser")}\n`,
        `# source_identity_limit,${MAX_SOURCE_IDENTITIES}\n`,
        "# source_identity_columns,source_id,source_slot,input_kind,device_family,device_name,palette_id\n",
        ...sourceIdentityHeaders,
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
