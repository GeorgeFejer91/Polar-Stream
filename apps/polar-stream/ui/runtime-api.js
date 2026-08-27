(() => {
  "use strict";

  const core = window.__TAURI__?.core;
  const isNative = Boolean(core?.invoke && core?.Channel);
  const isRenderer = new URLSearchParams(window.location.search).has("renderer");
  const mode = isNative ? "native" : isRenderer ? "renderer" : "browser-demo";
  const webBluetooth = window.PolarWebBluetooth;
  const vernierBluetooth = window.VernierWebBluetooth;
  const mockDevice = Object.freeze({
    id: "recorded-h10-preview",
    name: "Recorded Polar H10 preview",
    kind: "mock",
    detail: "Seamless loop of an anonymized 60-second ECG + ACC recording · no sensor required",
    sourceLabel: "RECORDED",
    rssi: null,
  });
  const demo = {
    player: null,
    callback: null,
    dataPromise: null,
    outputs: new Set(["raw_ecg", "raw_acc"]),
    config: null,
    metricPreviewIndex: -1,
  };
  const browserBluetoothOutputs = new Set([
    "raw_ecg",
    "raw_acc",
    "acc_magnitude",
    "heart_rate",
    "rr_interval",
    "acc_breathing_magnitude",
    "breathing_volume",
    "breathing_phase",
    "breathing_calibration",
    "breathing_axis_range",
    "breathing_signal_confidence",
    "breathing_signal_ready",
  ]);
  const browserVernierOutputs = new Set(["raw_force"]);
  const nativeSources = new Set();
  const sourcePaletteOverrides = new Map();
  let activeInput = null;

  function deliverInputEvent(callback, event, paletteId = null, sourceDefaults = null) {
    const source = { ...(sourceDefaults || {}), ...(event?.source || {}) };
    const effectivePaletteId = sourcePaletteOverrides.get(source.id) || paletteId;
    const palette = window.PolarSourcePalettes?.find((candidate) => candidate.id === effectivePaletteId) || null;
    const delivered = palette && source.id
      ? {
          ...event,
          source: {
            ...source,
            palette,
            color: palette.light.primary,
          },
        }
      : event;
    if (mode === "browser-demo") window.PolarBrowserSession?.publish(delivered);
    callback(delivered);
  }

  function browserBluetoothModule() {
    const status = webBluetooth?.supportStatus() || {
      supported: false,
      reason: "The browser Bluetooth adapter did not load.",
    };
    return Object.freeze({
      id: webBluetooth?.moduleId || "web-bluetooth-polar-h10",
      name: "Polar H10 via browser",
      kind: "web-bluetooth",
      detail: status.reason,
      sourceLabel: "EXPERIMENTAL",
      available: status.supported,
      rssi: null,
    });
  }

  function browserVernierModule() {
    const status = vernierBluetooth?.supportStatus() || {
      supported: false,
      reason: "The Go Direct browser Bluetooth adapter did not load.",
    };
    return Object.freeze({
      id: vernierBluetooth?.moduleId || "web-bluetooth-vernier-gdx",
      name: "Vernier Go Direct via browser",
      kind: "web-bluetooth-vernier",
      detail: status.reason,
      sourceLabel: "GDX · EXPERIMENTAL",
      available: status.supported,
      rssi: null,
    });
  }

  class RuntimeError extends Error {
    constructor(code, message, retryable = false) {
      super(message || "The native operation failed.");
      this.name = "RuntimeError";
      this.code = code || "NATIVE_OPERATION_FAILED";
      this.retryable = Boolean(retryable);
    }
  }

  function normalizeError(value) {
    if (value instanceof RuntimeError) return value;
    if (value && typeof value === "object") {
      return new RuntimeError(value.code, value.message || String(value), value.retryable);
    }
    return new RuntimeError("NATIVE_OPERATION_FAILED", String(value || "The native operation failed."));
  }

  async function invoke(command, payload) {
    try {
      return await core.invoke(command, payload);
    } catch (error) {
      throw normalizeError(error);
    }
  }

  async function previewDelay(value, milliseconds) {
    await new Promise((resolve) => window.setTimeout(resolve, milliseconds));
    return value;
  }

  function loadDemoData() {
    if (demo.dataPromise) return demo.dataPromise;
    demo.dataPromise = fetch("data/preview-recording.json", { cache: "no-cache" })
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        return response.json();
      })
      .then((value) => window.PolarPreviewFixture.validateFixture(value))
      .catch((error) => {
      demo.dataPromise = null;
      throw new RuntimeError(
        "RECORDED_PREVIEW_UNAVAILABLE",
        `The recorded Polar H10 preview could not be loaded: ${error.message || error}`,
        true,
      );
    });
    return demo.dataPromise;
  }

  function stopDemo({ notify = false } = {}) {
    demo.player?.stop();
    demo.player = null;
    if (notify && demo.callback) {
      demo.callback({
        kind: "connection",
        connected: false,
        streaming: false,
        simulated: true,
        deviceName: mockDevice.name,
        batteryPercent: null,
        message: "Recorded preview stopped",
      });
    }
    demo.callback = null;
    if (activeInput === "mock") activeInput = null;
  }

  async function startDemo(onEvent) {
    const data = await loadDemoData();
    stopDemo();
    demo.callback = onEvent;
    activeInput = "mock";
    demo.metricPreviewIndex = -1;
    onEvent({
      kind: "connection",
      connected: true,
      streaming: true,
      simulated: true,
      deviceName: mockDevice.name,
      batteryPercent: null,
      message: `Anonymized ${data.durationMs / 1000}-second Polar H10 recording streaming as a seamless local loop`,
    });
    demo.player = new window.PolarPreviewFixture.LoopPlayer(data, (event) => {
      onEvent(event);
      if (event.kind !== "accelerometer") return;
      const previews = window.PolarMetricPreviews?.metrics;
      if (!previews) return;
      const seconds = Number(event.sensorTimestampNs || 0) / 1_000_000_000;
      const index = Math.floor(seconds % (data.durationMs / 1000) / (data.durationMs / 1000) * 112);
      if (index === demo.metricPreviewIndex) return;
      demo.metricPreviewIndex = index;
      const invertPhase = Boolean(demo.config?.metricOptions?.breathing_phase?.processing?.breathing?.invertDirection);
      const values = [...demo.outputs]
        .filter((id) => !["raw_ecg", "raw_acc", "acc_magnitude", "heart_rate", "rr_interval"].includes(id))
        .map((id) => {
          const channel = previews[id]?.channels?.[0]?.values;
          if (!channel?.length) return null;
          let value = Number(channel[index % channel.length]);
          if (id === "breathing_phase" && invertPhase) value *= -1;
          return { id, value };
        })
        .filter(Boolean);
      if (values.length) onEvent({ kind: "metrics", values });
    }, { seamDurationMs: window.PolarPreviewFixture.defaultSeamDurationMs });
    demo.player.start();
  }

  const api = {
    isNative,
    isDemo: mode === "browser-demo",
    isBrowser: mode === "browser-demo",
    mode,
    getInputModules() {
      return mode === "browser-demo"
        ? [mockDevice, browserBluetoothModule(), browserVernierModule()]
        : [mockDevice];
    },
    isMockDevice(deviceId) {
      return deviceId === mockDevice.id;
    },
    isBrowserBluetoothDevice(deviceId) {
      return mode === "browser-demo" && deviceId === (webBluetooth?.moduleId || "web-bluetooth-polar-h10");
    },
    isBrowserVernierDevice(deviceId) {
      return mode === "browser-demo" && deviceId === (vernierBluetooth?.moduleId || "web-bluetooth-vernier-gdx");
    },
    outputSupport(metricId, inputKind) {
      if (inputKind === "web-bluetooth-vernier") {
        return browserVernierOutputs.has(metricId)
          ? { supported: true, reason: null }
          : { supported: false, reason: "Browser Go Direct input currently exposes its selected raw force channel." };
      }
      if (inputKind !== "web-bluetooth" || browserBluetoothOutputs.has(metricId)) {
        return { supported: true, reason: null };
      }
      return {
        supported: false,
        reason: "This derived processor currently requires the desktop app. Browser H10 input provides raw ECG/ACC, HR/RR, and the experimental ACC respiration waveform with readiness and confidence outputs.",
      };
    },
    formatError(error) {
      const normalized = normalizeError(error);
      return normalized.code === "NATIVE_OPERATION_FAILED"
        ? normalized.message
        : `${normalized.message} (${normalized.code})`;
    },
    async getBootstrap(fallback) {
      if (isNative) return invoke("get_bootstrap");
      return { ...fallback, platform: "browser demo" };
    },
    async migrateLegacyPreferences(legacy) {
      return isNative ? invoke("migrate_legacy_preferences", { legacy }) : legacy;
    },
    async saveDevicePalette(deviceId, paletteId) {
      if (!isNative) return;
      return invoke("save_device_palette", { deviceId, paletteId });
    },
    async scanDevices(preferredDeviceId = null) {
      if (isNative) return invoke("scan_devices", { preferredDeviceId });
      return previewDelay([], 180);
    },
    async connectDevice(deviceId, onEvent, paletteId = null) {
      if (deviceId === mockDevice.id) {
        if (paletteId) sourcePaletteOverrides.set("mock-source", paletteId);
        if (activeInput === "native" && isNative) {
          await Promise.all([...nativeSources].map((sourceId) => invoke("disconnect_device", { sourceId })));
          nativeSources.clear();
        }
        if (activeInput === "web-bluetooth") await webBluetooth.disconnect();
        return startDemo((event) => deliverInputEvent(onEvent, event, paletteId, {
          id: "mock-source", slot: "mock-source", label: "Recorded preview", inputKind: "mock",
        }));
      }
      stopDemo({ notify: activeInput === "mock" });
      if (this.isBrowserBluetoothDevice(deviceId)) {
        if (paletteId) sourcePaletteOverrides.set("browser-polar-source", paletteId);
        await webBluetooth.connect(
          (event) => deliverInputEvent(onEvent, event, paletteId, {
            id: "browser-polar-source", slot: "browser-polar-source", label: "Browser H10", inputKind: "web-bluetooth",
          }),
          demo.config || {},
        );
        activeInput = "web-bluetooth";
        return;
      }
      if (this.isBrowserVernierDevice(deviceId)) {
        const source = await vernierBluetooth.connect((event) => deliverInputEvent(onEvent, event, paletteId));
        return source;
      }
      if (!isNative) {
        throw new RuntimeError("BROWSER_BLE_UNAVAILABLE", "This input is not available in the browser.");
      }
      if (demo.config) await invoke("update_output_config", { config: demo.config });
      const events = new core.Channel();
      events.onmessage = onEvent;
      const result = await invoke("connect_device", { deviceId, paletteId, events });
      activeInput = "native";
      if (result?.id) nativeSources.add(result.id);
      return result;
    },
    async updateSourcePalette(sourceId, paletteId) {
      if (!isNative) {
        sourcePaletteOverrides.set(sourceId, paletteId);
        return null;
      }
      return invoke("update_source_palette", { sourceId, paletteId });
    },
    async attachActiveSources(onEvent) {
      if (!isNative) return [];
      const events = new core.Channel();
      events.onmessage = onEvent;
      const sources = await invoke("attach_active_sources", { events });
      for (const source of sources || []) {
        if (source?.id) nativeSources.add(source.id);
      }
      if (nativeSources.size) activeInput = "native";
      return sources || [];
    },
    async disconnectDevice(sourceId = null) {
      if (sourceId) sourcePaletteOverrides.delete(sourceId);
      if (activeInput === "mock") {
        stopDemo({ notify: true });
        return { emitted: true };
      }
      if (activeInput === "web-bluetooth") {
        activeInput = null;
        return webBluetooth.disconnect();
      }
      if (sourceId?.startsWith("browser-source-")) {
        return vernierBluetooth.disconnect(sourceId);
      }
      if (isNative && sourceId) {
        nativeSources.delete(sourceId);
        if (!nativeSources.size) activeInput = null;
        return invoke("disconnect_device", { sourceId });
      }
      if (isNative && nativeSources.size) {
        await Promise.all([...nativeSources].map((id) => invoke("disconnect_device", { sourceId: id })));
        nativeSources.clear();
      }
      activeInput = null;
      return undefined;
    },
    async updateOutputConfig(config) {
      if (mode === "browser-demo" && (config.lslEnabled || config.oscEnabled)) {
        throw new RuntimeError(
          "NATIVE_OUTPUT_REQUIRES_APP",
          "LSL and OSC outputs are available only in the installed Polar Stream app.",
          false,
        );
      }
      demo.config = structuredClone(config);
      demo.outputs = new Set(config.outputs || []);
      if (mode === "browser-demo") window.PolarBrowserSession?.configure(config);
      if (activeInput === "web-bluetooth") webBluetooth.updateConfig(config);
      if (!isNative) {
        return {
          streamName: config.streamName,
          lsl: "Installed app required · unavailable in browser",
          osc: "Installed app required · unavailable in browser",
          csv: config.csvEnabled ? "Recording in this browser tab" : "Off",
          audio: config.audioEnabled ? "Experimental PCM data modem" : "Off",
        };
      }
      if (activeInput === "mock") {
        return {
          streamName: config.streamName,
          lsl: config.lslEnabled ? "Recorded preview does not enter native LSL" : "Off",
          osc: config.oscEnabled ? "Recorded preview does not enter native OSC" : "Off",
          csv: config.csvEnabled ? "Recorded preview does not enter the native CSV writer" : "Off",
          audio: config.audioEnabled ? "Experimental PCM data modem" : "Off",
        };
      }
      return invoke("update_output_config", { config });
    },
    async openMetricCitation(metricId, url) {
      if (isNative) return invoke("open_metric_citation", { metricId, sourceUrl: url });
      const parsed = new URL(url);
      if (parsed.protocol !== "https:") {
        throw new RuntimeError("UNSAFE_CITATION_URL", "Only HTTPS citations can be opened.");
      }
      window.open(parsed.href, "_blank", "noopener,noreferrer");
    },
    async openLabRecorder() {
      if (isNative) return invoke("open_lab_recorder");
      throw new RuntimeError(
        "LAB_RECORDER_REQUIRES_APP",
        "The bundled LabRecorder is available only in the installed Polar Stream app.",
        false,
      );
    },
    async validateCustomFormula(formula) {
      if (isNative) return invoke("validate_custom_formula", { formula });
      if (!formula?.name?.trim() || !formula?.unit?.trim() || !formula?.expression?.trim()) {
        throw new RuntimeError("INVALID_FORMULA", "Name, unit, and expression are required.");
      }
      window.PolarFormulaPreview.parse(formula.expression);
      const source = window.PolarFormulaPreview.sourceMap[formula.source];
      if (!source) throw new RuntimeError("INVALID_FORMULA_SOURCE", "Choose a supported signal source.");
      return { normalized: formula, allowedVariables: source.variables, stateSamples: 0 };
    },
  };

  window.PolarRuntimeApi = Object.freeze(api);
})();
