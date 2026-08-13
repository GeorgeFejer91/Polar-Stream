(() => {
  "use strict";

  const core = window.__TAURI__?.core;
  const isNative = Boolean(core?.invoke && core?.Channel);
  const isRenderer = new URLSearchParams(window.location.search).has("renderer");
  const mode = isNative ? "native" : isRenderer ? "renderer" : "browser-demo";
  const mockDevice = Object.freeze({
    id: "neurokit-mock",
    name: "NeuroKit simulated input",
    kind: "mock",
    detail: "Generated locally · no Polar H10 required",
    sourceLabel: "SYNTHETIC",
    rssi: null,
  });
  const demo = {
    timer: null,
    callback: null,
    dataPromise: null,
    ecgIndex: 0,
    accIndex: 0,
    metricIndex: 0,
    ecgCarry: 0,
    outputs: new Set(["raw_ecg", "raw_acc"]),
    config: null,
  };
  let activeInput = null;

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
    if (window.PolarDemoData) return Promise.resolve(window.PolarDemoData);
    if (demo.dataPromise) return demo.dataPromise;
    demo.dataPromise = new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = "demo-data.js";
      script.async = true;
      script.addEventListener("load", () => {
        if (window.PolarDemoData) resolve(window.PolarDemoData);
        else reject(new RuntimeError("MOCK_DATA_INVALID", "The NeuroKit demo fixture did not initialize."));
      }, { once: true });
      script.addEventListener("error", () => reject(new RuntimeError(
        "MOCK_DATA_UNAVAILABLE",
        "The offline NeuroKit demo fixture could not be loaded.",
        true,
      )), { once: true });
      document.head.append(script);
    }).catch((error) => {
      demo.dataPromise = null;
      throw error;
    });
    return demo.dataPromise;
  }

  function circularSlice(values, start, count) {
    const result = new Array(count);
    for (let index = 0; index < count; index += 1) {
      result[index] = values[(start + index) % values.length];
    }
    return result;
  }

  function metricValue(data, id, index) {
    const fixture = data.metrics.values[id];
    if (fixture) return fixture[index % fixture.length];
    const phase = index / data.metrics.samplingRateHz;
    const wave = Math.sin(phase * 0.7);
    if (id === "mean_nn") return 60000 / (69 + wave * 3);
    if (id === "mean_heart_rate") return 69 + wave * 3;
    if (id === "sdnn") return 42 + wave * 5;
    if (id === "pnn50") return 18 + wave * 4;
    if (id === "sd1") return (31 + wave * 4) / Math.SQRT2;
    if (id === "coherence") return 0.62 + wave * 0.12;
    if (id === "coherence_confidence" || id === "breathing_dynamics_confidence") return 0.92;
    if (id === "heartmath_coherence") return 2.4 + wave * 0.5;
    if (id === "coherence_peak_frequency") return 0.1 + wave * 0.005;
    if (id.includes("coherence_") && id.includes("power")) return 1200 + wave * 120;
    if (id.startsWith("breath_interval_mean")) return 5 + wave * 0.2;
    if (id.startsWith("breath_amplitude_mean")) return 0.72 + wave * 0.05;
    if (id.includes("_sd")) return id.includes("interval") ? 0.22 + wave * 0.03 : 0.08 + wave * 0.01;
    if (id.includes("_cv")) return 0.12 + wave * 0.02;
    if (id.includes("acw50")) return 2 + wave * 0.2;
    if (id.includes("psd_slope")) return -1.1 + wave * 0.12;
    if (id.includes("lzc")) return 0.58 + wave * 0.05;
    if (id.includes("sampen")) return 1.2 + wave * 0.1;
    if (id.includes("mse")) return 4.2 + wave * 0.3;
    if (id === "excitement_score") return 0.52 + wave * 0.2;
    if (id === "excitometer") return 0.46 + wave * 0.15;
    if (id === "ecg_mean") return wave * 8;
    if (id === "ecg_rms" || id === "ecg_sd") return 180 + wave * 15;
    if (id === "ecg_peak_to_peak") return 1050 + wave * 60;
    if (id === "breathing_calibration") return 1;
    if (id === "breathing_axis_range") return 0.034;
    return 0.5 + wave * 0.1;
  }

  function emitDemoFrame(data) {
    if (!demo.callback) return;
    const intervalSeconds = 0.05;
    const exactEcgCount = data.ecg.samplingRateHz * intervalSeconds + demo.ecgCarry;
    const ecgCount = Math.floor(exactEcgCount);
    demo.ecgCarry = exactEcgCount - ecgCount;
    const microvolts = circularSlice(data.ecg.microvolts, demo.ecgIndex, ecgCount);
    demo.ecgIndex = (demo.ecgIndex + ecgCount) % data.ecg.microvolts.length;

    const accCount = Math.round(data.accelerometer.samplingRateHz * intervalSeconds);
    const [x, y, z] = data.accelerometer.milligravity;
    const samples = Array.from({ length: accCount }, (_, offset) => {
      const index = (demo.accIndex + offset) % x.length;
      return { xMg: x[index], yMg: y[index], zMg: z[index] };
    });
    demo.accIndex = (demo.accIndex + accCount) % x.length;

    const metricIndex = demo.metricIndex % (data.durationSeconds * data.metrics.samplingRateHz);
    const invertPhase = Boolean(
      demo.config?.metricOptions?.breathing_phase?.processing?.breathing?.invertDirection,
    );
    const values = [...demo.outputs]
      .filter((id) => !["raw_ecg", "raw_acc", "acc_magnitude"].includes(id))
      .map((id) => {
        const value = metricValue(data, id, metricIndex);
        return { id, value: id === "breathing_phase" && invertPhase ? -value : value };
      });
    demo.metricIndex = (demo.metricIndex + 1) % (data.durationSeconds * data.metrics.samplingRateHz);

    demo.callback({ kind: "ecg", sensorTimestampNs: 0, microvolts });
    demo.callback({ kind: "accelerometer", sensorTimestampNs: 0, samples });
    if (values.length) demo.callback({ kind: "metrics", values });
  }

  function stopDemo({ notify = false } = {}) {
    if (demo.timer) window.clearInterval(demo.timer);
    demo.timer = null;
    if (notify && demo.callback) {
      demo.callback({
        kind: "connection",
        connected: false,
        streaming: false,
        simulated: true,
        deviceName: mockDevice.name,
        batteryPercent: null,
        message: "Synthetic input stopped",
      });
    }
    demo.callback = null;
    if (activeInput === "mock") activeInput = null;
  }

  async function startDemo(onEvent) {
    const data = await loadDemoData();
    stopDemo();
    demo.callback = onEvent;
    demo.ecgIndex = 0;
    demo.accIndex = 0;
    demo.metricIndex = 0;
    demo.ecgCarry = 0;
    activeInput = "mock";
    onEvent({
      kind: "connection",
      connected: true,
      streaming: true,
      simulated: true,
      deviceName: mockDevice.name,
      batteryPercent: null,
      message: `Synthetic ${data.source.library} ${data.source.version} fixture streaming locally`,
    });
    emitDemoFrame(data);
    demo.timer = window.setInterval(() => emitDemoFrame(data), 50);
  }

  const api = {
    isNative,
    isDemo: mode === "browser-demo",
    mode,
    getInputModules() {
      return [mockDevice];
    },
    isMockDevice(deviceId) {
      return deviceId === mockDevice.id;
    },
    formatError(error) {
      const normalized = normalizeError(error);
      return normalized.code === "NATIVE_OPERATION_FAILED"
        ? normalized.message
        : `${normalized.message} (${normalized.code})`;
    },
    async getBootstrap(fallback) {
      return isNative ? invoke("get_bootstrap") : { ...fallback, platform: "browser demo" };
    },
    async migrateLegacyPreferences(legacy) {
      return isNative ? invoke("migrate_legacy_preferences", { legacy }) : legacy;
    },
    async scanDevices() {
      if (isNative) return invoke("scan_devices");
      return previewDelay([], 180);
    },
    async connectDevice(deviceId, onEvent) {
      if (deviceId === mockDevice.id) {
        if (activeInput === "native" && isNative) await invoke("disconnect_device");
        return startDemo(onEvent);
      }
      stopDemo({ notify: activeInput === "mock" });
      if (!isNative) {
        throw new RuntimeError("BROWSER_BLE_UNAVAILABLE", "This browser demo uses the NeuroKit mock input.");
      }
      if (demo.config) await invoke("update_output_config", { config: demo.config });
      const events = new core.Channel();
      events.onmessage = onEvent;
      const result = await invoke("connect_device", { deviceId, events });
      activeInput = "native";
      return result;
    },
    async disconnectDevice() {
      if (activeInput === "mock") {
        stopDemo({ notify: true });
        return { emitted: true };
      }
      activeInput = null;
      return isNative ? invoke("disconnect_device") : undefined;
    },
    async updateOutputConfig(config) {
      demo.config = structuredClone(config);
      demo.outputs = new Set(config.outputs || []);
      if (activeInput === "mock" || !isNative) {
        return {
          streamName: config.streamName,
          lsl: config.lslEnabled ? "Demo only · no LSL outlet is opened" : "Off",
          osc: config.oscEnabled ? "Demo only · no UDP packets are sent" : "Off",
        };
      }
      return invoke("update_output_config", { config });
    },
    async openMetricCitation(metricId, url) {
      if (isNative) return invoke("open_metric_citation", { metricId });
      const parsed = new URL(url);
      if (parsed.protocol !== "https:") {
        throw new RuntimeError("UNSAFE_CITATION_URL", "Only HTTPS citations can be opened.");
      }
      window.open(parsed.href, "_blank", "noopener,noreferrer");
    },
  };

  window.PolarRuntimeApi = Object.freeze(api);
})();
