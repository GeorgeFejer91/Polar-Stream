(() => {
  "use strict";

  const nativeCore = window.__TAURI__?.core;
  const isNative = Boolean(nativeCore?.invoke && nativeCore?.Channel);
  const invoke = nativeCore?.invoke?.bind(nativeCore);
  const NativeChannel = nativeCore?.Channel;
  const preferences = window.PolarPreferences;
  const isInterfaceRenderer = new URLSearchParams(window.location.search).has("renderer");

  const evidenceLinks = {
    hrv: ["Shaffer & Ginsberg (2017)", "https://www.frontiersin.org/journals/public-health/articles/10.3389/fpubh.2017.00258/full"],
    breathing: ["Drummond et al. (2021)", "https://consensus.app/papers/details/9389301a991e52ee9210f51939d318d7/?utm_source=unknown"],
    complexity: ["Bará et al. (2024)", "https://consensus.app/papers/details/de562a29bb8454eda201852b544fd9d7/?utm_source=unknown"],
    resonance: ["Sévoz-Couche & Laborde (2022)", "https://www.sciencedirect.com/science/article/abs/pii/S0149763422000653"],
    quality: ["Smital et al. (2020)", "https://consensus.app/papers/details/ad2a724fefd55d25baee823438fc672e/?utm_source=unknown"],
    stress: ["Immanuel et al. (2023)", "https://consensus.app/papers/details/14838ea9a9045710b4a676dbb7d595aa/?utm_source=unknown"],
    exciteometer: ["Excite-O-Meter source implementation", "https://github.com/luisqtr/exciteometer/blob/main/docs/1_UserManual.md#scientific-disclaimer"],
  };
  const fallbackMetric = (id, streamSuffix, label, unit, category, source = "hrv", detail = "Real-time derived metric", raw = false, normalizable = true, rateHz = 0) => ({
    id, streamSuffix, label, detail, unit, category, raw, normalizable, rateHz,
    evidence: raw ? "device signal" : "research metric",
    citationLabel: evidenceLinks[source][0], citationUrl: evidenceLinks[source][1],
    keywords: `${label} ${category}`.toLowerCase(),
    explainer: `${label} is exposed as a descriptive research signal using the formula named in this card. Its physiological meaning depends on signal quality, recording context and the cited method; it is not a diagnosis or a standalone emotional-state measure.`,
  });
  const fallbackCatalog = [
    fallbackMetric("raw_ecg", "rawECG", "Raw ECG", "µV", "Raw signals", "quality", "Unfiltered H10 voltage · 130 Hz", true, false, 130),
    fallbackMetric("raw_acc", "rawACC", "Raw accelerometer", "mg", "Raw signals", "breathing", "X, Y and Z · 200 Hz", true, false, 200),
    fallbackMetric("acc_magnitude", "accMagnitude", "ACC magnitude", "g", "Raw signals", "breathing", "√(x²+y²+z²)", false, true, 200),
    ...[["ecg_mean","ecgMean","ECG window mean"],["ecg_rms","ecgRms","ECG RMS amplitude"],["ecg_peak_to_peak","ecgPeakToPeak","ECG peak-to-peak"],["ecg_sd","ecgSd","ECG standard deviation"]].map(([id,suffix,label]) => fallbackMetric(id,suffix,label,"µV","ECG features","quality","Five-second rolling signal feature",false,true,2)),
    fallbackMetric("heart_rate", "heartRate", "Heart rate", "bpm", "Heart rate", "hrv", "H10 device-derived", false, true),
    fallbackMetric("rr_interval", "rrInterval", "RR interval", "ms", "Heart rate", "hrv", "Accepted beat-to-beat interval"),
    fallbackMetric("mean_nn", "meanNN", "Mean NN interval", "ms", "Heart rate"),
    fallbackMetric("mean_heart_rate", "meanHeartRate", "Mean heart rate", "bpm", "Heart rate"),
    ...[["rmssd","rmssd","RMSSD","ms"],["ln_rmssd","lnRMSSD","lnRMSSD","ln(ms)"],["sdnn","sdnn","SDNN","ms"],["pnn50","pNN50","pNN50","%"],["sd1","sd1","Poincaré SD1","ms"]].map(([id,suffix,label,unit]) => fallbackMetric(id,suffix,label,unit,"HRV & relaxation")),
    ...[["coherence","coherence","Normalized coherence","0–1"],["coherence_confidence","coherenceConfidence","Coherence confidence","0–1"],["heartmath_coherence","heartMathCoherence","HeartMath-style coherence ratio","ratio"],["coherence_peak_frequency","coherencePeakFrequency","Coherence peak frequency","Hz"],["coherence_peak_power","coherencePeakPower","Coherence peak-band power","ms²"],["coherence_total_power","coherenceTotalPower","Coherence total power","ms²"]].map(([id,suffix,label,unit]) => fallbackMetric(id,suffix,label,unit,"Coherence","resonance")),
    fallbackMetric("breathing_volume", "breathingVolume", "ACC breathing waveform", "0–1", "Breathing", "breathing", "Calibrated chest-motion projection", false, true, 20),
    fallbackMetric("breathing_phase", "breathingPhase", "Breath phase classifier", "class", "Breathing", "breathing", "+1 inhale · −1 exhale · 0 pause · −2 bad signal", false, false, 20),
    fallbackMetric("breathing_calibration", "breathingCalibration", "Breathing calibration", "0–1", "Breathing", "breathing", "Principal-axis calibration progress", false, false, 4),
    fallbackMetric("breathing_axis_range", "breathingAxisRange", "Breathing axis range", "g", "Breathing", "breathing"),
    fallbackMetric("breathing_rate", "breathingRate", "Breathing rate", "breaths/min", "Breathing", "breathing"),
    fallbackMetric("breathing_dynamics_confidence", "breathingDynamicsConfidence", "Breathing-dynamics confidence", "0–1", "Breathing dynamics", "complexity"),
    ...["mean","sd","cv","acw50","psd_slope","lzc","sampen","mse"].flatMap((feature) => ["interval","amplitude"].map((kind) => {
      const names = { mean:"mean", sd:"SD", cv:"CV", acw50:"ACW50", psd_slope:"PSD slope", lzc:"Lempel–Ziv", sampen:"sample entropy", mse:"multiscale entropy" };
      const units = kind === "interval" ? {mean:"s",sd:"s",cv:"ratio",acw50:"breaths",psd_slope:"slope",lzc:"0–1",sampen:"entropy",mse:"entropy AUC"} : {mean:"0–1",sd:"0–1",cv:"ratio",acw50:"breaths",psd_slope:"slope",lzc:"0–1",sampen:"entropy",mse:"entropy AUC"};
      const camel = feature.split("_").map((part,index) => index ? part[0].toUpperCase()+part.slice(1) : part).join("");
      return fallbackMetric(`breath_${kind}_${feature}`, `breath${kind[0].toUpperCase()+kind.slice(1)}${camel[0].toUpperCase()+camel.slice(1)}`, `Breath ${kind} ${names[feature]}`, units[feature], "Breathing dynamics", "complexity");
    })),
    fallbackMetric("excitement_score", "excitementScore", "Excite-O-Meter excitement score", "0–1", "Excitation (experimental)", "exciteometer", "1 − mean[Φ(zRR), Φ(zRMSSD)] · live provisional", false, false),
    fallbackMetric("excitometer", "excitometer", "Activation composite (experimental)", "0–1", "Excitation (experimental)", "stress", "Within-session HR ↑ plus lnRMSSD ↓"),
  ];

  const visualDefinitions = {
    raw_ecg: { label: "Raw ECG", unit: "µV", rate: 130, color: "#d85151", symmetric: true },
    acc_x: { label: "Accelerometer · X", unit: "mg", rate: 200, color: "#3b78aa", symmetric: true, parent: "raw_acc" },
    acc_y: { label: "Accelerometer · Y", unit: "mg", rate: 200, color: "#168259", symmetric: true, parent: "raw_acc" },
    acc_z: { label: "Accelerometer · Z", unit: "mg", rate: 200, color: "#a66d19", symmetric: true, parent: "raw_acc" },
    heart_rate: { label: "Heart rate", unit: "bpm", rate: 1, color: "#d85151" },
    rr_interval: { label: "RR interval", unit: "ms", rate: 2, color: "#6c62a8" },
    acc_magnitude: { label: "ACC magnitude", unit: "g", rate: 200, color: "#3b78aa" },
    rmssd: { label: "RMSSD", unit: "ms", rate: 1, color: "#168259" },
  };

  class RingBuffer {
    constructor(capacity = 4096) {
      this.values = new Float32Array(capacity);
      this.capacity = capacity;
      this.length = 0;
      this.cursor = 0;
    }

    push(value) {
      if (!Number.isFinite(value)) return;
      this.values[this.cursor] = value;
      this.cursor = (this.cursor + 1) % this.capacity;
      this.length = Math.min(this.length + 1, this.capacity);
    }

    pushMany(values) {
      for (const value of values) this.push(Number(value));
    }

    tail(count) {
      const size = Math.min(this.length, count);
      const result = new Float32Array(size);
      const start = (this.cursor - size + this.capacity) % this.capacity;
      const first = Math.min(size, this.capacity - start);
      result.set(this.values.subarray(start, start + first));
      if (first < size) result.set(this.values.subarray(0, size - first), first);
      return result;
    }

    latest() {
      return this.length ? this.values[(this.cursor - 1 + this.capacity) % this.capacity] : null;
    }

    clear() {
      this.length = 0;
      this.cursor = 0;
    }
  }

  const buffers = Object.fromEntries(Object.keys(visualDefinitions).map((id) => [id, new RingBuffer()]));
  const elements = {};
  const ids = [
    "app-state-dot", "app-state-text", "platform-label", "input-state", "connection-card",
    "device-name", "connection-detail", "disconnect-button", "connection-meta", "battery-value",
    "scan-button", "scan-caption", "device-list", "activity-list", "output-state", "raw-ecg-value",
    "raw-acc-x", "raw-acc-y", "raw-acc-z", "ecg-spark", "stream-name", "lsl-toggle", "osc-toggle",
    "lsl-detail", "osc-detail", "included-count", "output-chips", "open-output-dialog", "visual-source",
    "visual-current", "visual-unit", "visual-label", "render-rate", "chart-shell", "signal-canvas",
    "chart-empty", "y-max", "y-min", "footer-status", "sample-counter", "output-dialog",
    "metric-options", "metric-detail", "dialog-output-status", "save-metric-output", "toast-region",
    "stream-name-preview", "metric-search", "metric-filters", "metric-library-summary",
    "adjust-visual", "visual-window-label", "visual-scale-label", "module-dialog", "module-dialog-title",
    "module-dialog-intro", "module-settings", "module-dialog-status", "save-module-settings",
  ];
  for (const id of ids) elements[id] = document.getElementById(id);

  const app = {
    connected: false,
    connecting: false,
    scanning: false,
    configuring: false,
    catalog: fallbackCatalog,
    outputs: new Set(["raw_ecg", "raw_acc"]),
    metricOptions: {},
    visualNormalizers: {},
    selectedMetricId: null,
    editingModuleId: null,
    moduleDraft: null,
    metricFilter: "All",
    metricSearch: "",
    streamName: "Polar-H10",
    selectedVisual: "raw_ecg",
    sampleCount: 0,
    demoTimer: null,
    demoPhase: 0,
    outputSequence: 0,
    connectionGeneration: 0,
    currentDeviceId: null,
    pendingDevice: null,
    devices: [],
    preferences: preferences.load(),
    activity: [{ time: "NOW", message: "Bluetooth interface ready" }],
  };

  function normalizeStreamBase(value) {
    if (typeof value !== "string") return null;
    let normalized = "";
    let separatorPending = false;
    for (const character of String(value).trim()) {
      if (/[A-Za-z0-9]/.test(character)) {
        if (separatorPending && normalized && !normalized.endsWith("_") && !normalized.endsWith("-")) {
          normalized += "_";
        }
        normalized += character;
        separatorPending = false;
      } else if (character === "-") {
        normalized += character;
        separatorPending = false;
      } else {
        separatorPending = true;
      }
    }
    normalized = normalized.replace(/^[_-]+|[_-]+$/g, "");
    return normalized && normalized.length <= 64 ? normalized : null;
  }

  function streamOutputName(metric, value = app.streamName) {
    const base = normalizeStreamBase(value);
    const suffix = metric.streamSuffix
      || fallbackCatalog.find((candidate) => candidate.id === metric.id)?.streamSuffix
      || metric.id;
    return base ? `${base}_${suffix}` : `—_${suffix}`;
  }

  function setTopStatus(message, state = "idle") {
    elements["app-state-text"].textContent = message;
    elements["app-state-dot"].className = `state-dot${state === "idle" ? "" : ` ${state}`}`;
    elements["footer-status"].textContent = message;
  }

  function addActivity(message) {
    const time = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    app.activity.unshift({ time, message });
    app.activity.length = Math.min(app.activity.length, 3);
    elements["activity-list"].replaceChildren(...app.activity.map((item) => {
      const row = document.createElement("li");
      const stamp = document.createElement("time");
      const text = document.createElement("span");
      stamp.textContent = item.time;
      text.textContent = item.message;
      row.append(stamp, text);
      return row;
    }));
  }

  function toast(message, error = false) {
    const node = document.createElement("div");
    node.className = `toast${error ? " error" : ""}`;
    node.textContent = message;
    elements["toast-region"].append(node);
    window.setTimeout(() => node.remove(), 4200);
  }

  async function initialize() {
    let bootstrap = {
      config: { streamName: "Polar-H10", lslEnabled: false, oscEnabled: false, outputs: ["raw_ecg", "raw_acc"], metricOptions: {} },
      platform: "browser preview",
      metricCatalog: fallbackCatalog,
    };
    if (isNative) {
      try {
        bootstrap = await invoke("get_bootstrap");
      } catch (error) {
        toast(String(error), true);
      }
    }

    app.catalog = bootstrap.metricCatalog || fallbackCatalog;
    const initialConfig = app.preferences.outputConfig || bootstrap.config || {};
    app.outputs = new Set(initialConfig.outputs || ["raw_ecg", "raw_acc"]);
    app.metricOptions = structuredClone(initialConfig.metricOptions || {});
    installCatalogVisuals();
    app.streamName = normalizeStreamBase(app.preferences.streamName)
      || normalizeStreamBase(initialConfig.streamName)
      || "Polar-H10";
    elements["stream-name"].value = app.streamName;
    elements["lsl-toggle"].checked = Boolean(initialConfig.lslEnabled);
    elements["osc-toggle"].checked = Boolean(initialConfig.oscEnabled);
    elements["platform-label"].textContent = isInterfaceRenderer ? "RENDERER" : String(bootstrap.platform || "local").toUpperCase();
    if (!isNative) {
      elements["scan-caption"].textContent = isInterfaceRenderer
        ? "Deterministic background render"
        : "Interactive browser preview";
    }

    renderMetricFilters();
    renderMetricOptions();
    renderOutputs();
    installInteractions();
    await configureOutputs({ quiet: true });
    resizeCanvas();
    if (!isInterfaceRenderer) window.requestAnimationFrame(drawFrame);
    if (app.preferences.lastDevice) void scanDevices({ automatic: true });
  }

  function installInteractions() {
    elements["scan-button"].addEventListener("click", () => scanDevices());
    elements["disconnect-button"].addEventListener("click", disconnectDevice);
    elements["lsl-toggle"].addEventListener("change", configureOutputs);
    elements["osc-toggle"].addEventListener("change", configureOutputs);

    let nameTimer;
    elements["stream-name"].addEventListener("input", () => {
      app.streamName = elements["stream-name"].value;
      renderOutputs();
      window.clearTimeout(nameTimer);
      nameTimer = window.setTimeout(configureOutputs, 320);
    });

    elements["open-output-dialog"].addEventListener("click", () => {
      app.selectedMetricId = null;
      renderMetricOptions();
      renderMetricDetail();
      elements["output-dialog"].showModal();
    });
    elements["save-metric-output"].addEventListener("click", () => {
      const metric = app.catalog.find((candidate) => candidate.id === app.selectedMetricId);
      if (!metric || app.outputs.has(metric.id)) return;
      app.outputs.add(metric.id);
      renderOutputs();
      configureOutputs();
      elements["output-dialog"].close();
      toast(`${metric.label} added as ${streamOutputName(metric)}`);
    });
    elements["metric-search"].addEventListener("input", () => {
      app.metricSearch = elements["metric-search"].value.trim().toLowerCase();
      renderMetricOptions();
    });
    elements["metric-filters"].addEventListener("click", (event) => {
      const button = event.target.closest("button[data-category]");
      if (!button) return;
      app.metricFilter = button.dataset.category;
      renderMetricFilters();
      renderMetricOptions();
    });
    elements["visual-source"].addEventListener("change", () => {
      app.selectedVisual = elements["visual-source"].value;
      updateVisualLabels();
    });
    elements["adjust-visual"].addEventListener("click", () => openModuleSettings(optionIdForVisual(app.selectedVisual)));
    elements["save-module-settings"].addEventListener("click", saveModuleSettings);

    const observer = new ResizeObserver(resizeCanvas);
    observer.observe(elements["chart-shell"]);
  }

  async function scanDevices({ automatic = false } = {}) {
    if (app.scanning) return;
    app.scanning = true;
    elements["scan-button"].disabled = true;
    elements["scan-button"].classList.add("scanning");
    elements["scan-button"].querySelector("span").textContent = "Scanning…";
    elements["input-state"].textContent = "Scanning";
    setTopStatus("Scanning for Polar sensors", "working");
    addActivity(automatic ? "Looking for last used sensor" : "BLE scan started");

    try {
      const devices = isNative
        ? await invoke("scan_devices")
        : await new Promise((resolve) => window.setTimeout(() => resolve([
            { id: "preview-h10-a", name: "Polar H10 8F3A2C1B", rssi: -48 },
            { id: "preview-h10-b", name: "Polar H10 4D9E7A20", rssi: -63 },
          ]), 850));
      app.devices = devices;
      renderDevices(devices);
      const count = devices.length;

      if (automatic && app.preferences.lastDevice) {
        const exact = devices.find((device) => device.id === app.preferences.lastDevice.id);
        const nameMatches = devices.filter((device) => device.name === app.preferences.lastDevice.name);
        const preferredDevice = exact || (nameMatches.length === 1 ? nameMatches[0] : null);
        if (preferredDevice) {
          addActivity(`Last used sensor found · ${preferredDevice.name}`);
          await connectDevice(preferredDevice, { automatic: true });
          return;
        }
        elements["input-state"].textContent = count ? `${count} found` : "None found";
        setTopStatus("Last used sensor unavailable · choose below");
        addActivity("Last used sensor was not found");
        return;
      }

      elements["input-state"].textContent = count ? `${count} found` : "None found";
      setTopStatus(app.connected ? "Sensor connected · choose another to switch" : count ? "Choose a sensor to connect" : "No Polar sensor found", app.connected ? "connected" : "idle");
      addActivity(count ? `${count} compatible sensor${count === 1 ? "" : "s"} found` : "Scan finished with no sensors");
    } catch (error) {
      const message = String(error);
      setTopStatus("Bluetooth scan failed", "error");
      elements["input-state"].textContent = "Error";
      addActivity(message);
      toast(message, true);
    } finally {
      app.scanning = false;
      elements["scan-button"].disabled = false;
      elements["scan-button"].classList.remove("scanning");
      elements["scan-button"].querySelector("span").textContent = "Scan again";
    }
  }

  function renderDevices(devices) {
    if (!devices.length) {
      const empty = document.createElement("div");
      empty.className = "empty-state";
      const orbit = document.createElement("span");
      orbit.className = "empty-orbit";
      const message = document.createElement("p");
      message.textContent = "No Polar sensors found.";
      const hint = document.createElement("small");
      hint.textContent = "Check Bluetooth, wear the strap, then scan again.";
      empty.append(orbit, message, hint);
      elements["device-list"].replaceChildren(empty);
      return;
    }

    const rows = devices.map((device) => {
      const isCurrent = app.connected && app.currentDeviceId === device.id;
      const isPending = app.pendingDevice?.id === device.id;
      const isPreferred = app.preferences.lastDevice?.id === device.id;
      const button = document.createElement("button");
      button.className = `device-row${isCurrent ? " current" : ""}${isPreferred ? " preferred" : ""}`;
      button.type = "button";
      button.disabled = app.connecting || isCurrent;
      button.addEventListener("click", () => connectDevice(device));

      const icon = document.createElement("span");
      icon.className = "device-icon";
      icon.textContent = "H10";
      const copy = document.createElement("span");
      copy.className = "device-copy";
      const nameLine = document.createElement("span");
      nameLine.className = "device-name-line";
      const name = document.createElement("strong");
      name.textContent = device.name;
      nameLine.append(name);
      if (isPreferred) {
        const badge = document.createElement("span");
        badge.className = "preference-badge";
        badge.textContent = "LAST USED";
        nameLine.append(badge);
      }
      const id = document.createElement("small");
      id.textContent = device.id;
      copy.append(nameLine, id);
      const rssi = document.createElement("span");
      rssi.className = "rssi";
      rssi.textContent = isCurrent
        ? "Connected"
        : isPending
          ? "Connecting…"
          : device.rssi == null
            ? "Connect →"
            : `${device.rssi} dBm  →`;
      button.append(icon, copy, rssi);
      return button;
    });
    elements["device-list"].replaceChildren(...rows);
  }

  async function connectDevice(device, { automatic = false } = {}) {
    const generation = ++app.connectionGeneration;
    app.connecting = true;
    app.pendingDevice = device;
    renderDevices(app.devices);
    setTopStatus(automatic ? "Reconnecting to last used Polar H10" : "Connecting to Polar H10", "working");
    elements["input-state"].textContent = "Connecting";
    elements["device-name"].textContent = device.name;
    elements["connection-detail"].textContent = "Opening the low-energy connection…";
    addActivity(`${automatic ? "Reconnecting" : "Connecting"} to ${device.name}`);

    try {
      if (isNative) {
        const channel = new NativeChannel();
        channel.onmessage = (event) => {
          if (generation === app.connectionGeneration) handleNativeEvent(event, device);
        };
        await invoke("connect_device", { deviceId: device.id, events: channel });
      } else {
        await new Promise((resolve) => window.setTimeout(resolve, 450));
        handleNativeEvent({
          kind: "connection", connected: true, streaming: true, deviceName: device.name,
          batteryPercent: 86, message: "Raw ECG and accelerometer are streaming",
        }, device);
        startDemoSignal();
      }
    } catch (error) {
      if (generation !== app.connectionGeneration) return;
      app.connecting = false;
      app.pendingDevice = null;
      setTopStatus("Connection failed", "error");
      elements["input-state"].textContent = "Error";
      elements["connection-detail"].textContent = String(error);
      addActivity(String(error));
      toast(String(error), true);
      renderDevices(app.devices);
    }
  }

  async function disconnectDevice() {
    const previousGeneration = app.connectionGeneration;
    app.connectionGeneration += 1;
    try {
      if (isNative) await invoke("disconnect_device");
      stopDemoSignal();
      handleNativeEvent({
        kind: "connection", connected: false, streaming: false,
        deviceName: elements["device-name"].textContent, batteryPercent: null, message: "Disconnected",
      });
    } catch (error) {
      app.connectionGeneration = previousGeneration;
      toast(String(error), true);
    }
  }

  function handleNativeEvent(event, device = null) {
    switch (event.kind) {
      case "status":
        setTopStatus(event.message, "working");
        elements["connection-detail"].textContent = event.message;
        addActivity(event.message);
        break;
      case "connection":
        updateConnection(event, device);
        break;
      case "ecg":
        ingestEcg(event.microvolts || []);
        break;
      case "accelerometer":
        ingestAccelerometer(event.samples || []);
        break;
      case "metrics":
        ingestMetrics(event);
        break;
      case "error":
        addActivity(event.message);
        toast(event.message, true);
        break;
      default:
        break;
    }
  }

  function updateConnection(event, device = null) {
    app.connected = Boolean(event.connected);
    app.connecting = false;
    app.pendingDevice = null;
    if (app.connected) {
      resetMeasurementVisuals();
      const connectedDevice = device || app.devices.find((candidate) => candidate.name === event.deviceName);
      app.currentDeviceId = connectedDevice?.id || null;
      if (connectedDevice) {
        app.preferences = preferences.saveLastDevice(connectedDevice);
      }
    } else {
      app.currentDeviceId = null;
    }
    renderDevices(app.devices);
    elements["connection-card"].classList.toggle("connected", app.connected);
    elements["disconnect-button"].hidden = !app.connected;
    elements["connection-meta"].hidden = !app.connected;
    elements["device-name"].textContent = app.connected ? event.deviceName : "No sensor connected";
    elements["connection-detail"].textContent = app.connected ? event.message : "Scan for a nearby chest strap.";
    elements["battery-value"].textContent = event.batteryPercent == null ? "—" : `${event.batteryPercent}%`;
    elements["input-state"].textContent = app.connected ? "Streaming" : "Idle";
    setTopStatus(app.connected ? "Sensor connected · streams live" : "Ready to connect", app.connected ? "connected" : "idle");
    addActivity(app.connected ? `${event.deviceName} connected` : "Sensor disconnected");
  }

  function ingestEcg(values) {
    buffers.raw_ecg.pushMany(values);
    app.sampleCount += values.length;
    const latest = buffers.raw_ecg.latest();
    elements["raw-ecg-value"].textContent = formatValue(latest, 0);
    updateSparkline();
    updateSampleCounter();
  }

  function ingestAccelerometer(samples) {
    if (!samples.length) return;
    for (const sample of samples) {
      const x = Number(sample.xMg ?? sample.x_mg ?? 0);
      const y = Number(sample.yMg ?? sample.y_mg ?? 0);
      const z = Number(sample.zMg ?? sample.z_mg ?? 0);
      buffers.acc_x.push(x);
      buffers.acc_y.push(y);
      buffers.acc_z.push(z);
      ensureBuffer("acc_magnitude").push(visualValue("acc_magnitude", Math.hypot(x, y, z) / 1000));
    }
    const last = samples[samples.length - 1];
    elements["raw-acc-x"].textContent = formatValue(last.xMg ?? last.x_mg, 0);
    elements["raw-acc-y"].textContent = formatValue(last.yMg ?? last.y_mg, 0);
    elements["raw-acc-z"].textContent = formatValue(last.zMg ?? last.z_mg, 0);
    app.sampleCount += samples.length;
    updateSampleCounter();
  }

  function ingestMetrics(event) {
    if (Array.isArray(event.values)) {
      for (const metric of event.values) ensureBuffer(metric.id).push(visualValue(metric.id, Number(metric.value)));
      return;
    }
    // Backward compatibility for early v0.1 event payloads.
    ensureBuffer("heart_rate").push(visualValue("heart_rate", event.heartRateBpm ?? event.heart_rate_bpm));
    for (const value of event.rrIntervalsMs ?? event.rr_intervals_ms ?? []) {
      ensureBuffer("rr_interval").push(visualValue("rr_interval", value));
    }
    const rmssd = event.rmssdMs ?? event.rmssd_ms;
    if (rmssd != null) ensureBuffer("rmssd").push(visualValue("rmssd", rmssd));
  }

  function updateSampleCounter() {
    elements["sample-counter"].textContent = `${app.sampleCount.toLocaleString()} samples`;
  }

  function installCatalogVisuals() {
    const palette = {
      "Raw signals": "#d85151", "ECG features": "#b24e68", "Heart rate": "#d85151",
      "HRV & relaxation": "#168259", Coherence: "#6c62a8", Breathing: "#3b78aa",
      "Breathing dynamics": "#a66d19", "Excitation (experimental)": "#b94b40",
    };
    for (const metric of app.catalog) {
      if (!visualDefinitions[metric.id]) {
        visualDefinitions[metric.id] = {
          label: metric.label, unit: metric.unit, rate: Number(metric.rateHz) || 1,
          color: palette[metric.category] || "#168259",
          symmetric: metric.id === "breathing_phase" || /^ecg_(mean|sd)$/.test(metric.id),
        };
      }
      ensureBuffer(metric.id);
    }
  }

  function ensureBuffer(id) {
    if (!buffers[id]) buffers[id] = new RingBuffer();
    return buffers[id];
  }

  function renderMetricFilters() {
    const categories = ["All", ...new Set(app.catalog.map((metric) => metric.category))];
    const buttons = categories.map((category) => {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.category = category;
      button.className = category === app.metricFilter ? "active" : "";
      button.textContent = category;
      button.setAttribute("aria-pressed", String(category === app.metricFilter));
      return button;
    });
    elements["metric-filters"].replaceChildren(...buttons);
  }

  function renderMetricOptions() {
    const visible = app.catalog.filter((metric) => {
      const categoryMatches = app.metricFilter === "All" || metric.category === app.metricFilter;
      const haystack = `${metric.label} ${metric.detail} ${metric.category} ${metric.keywords || ""}`.toLowerCase();
      return categoryMatches && (!app.metricSearch || haystack.includes(app.metricSearch));
    });
    const options = visible.map((metric) => {
      const option = document.createElement("button");
      option.type = "button";
      option.className = `metric-option${app.selectedMetricId === metric.id ? " selected" : ""}`;
      option.setAttribute("aria-pressed", String(app.selectedMetricId === metric.id));
      const mark = document.createElement("span");
      mark.textContent = metric.raw ? "RAW" : metric.category.startsWith("Excitation") ? "EXP" : "FX";
      const copy = document.createElement("span");
      copy.className = "metric-option-copy";
      const heading = document.createElement("span");
      heading.className = "metric-option-heading";
      const name = document.createElement("strong");
      name.textContent = metric.label;
      const evidence = document.createElement("em");
      evidence.textContent = metric.evidence || "research metric";
      heading.append(name, evidence);
      const detail = document.createElement("small");
      detail.textContent = `${metric.detail} · ${metric.unit} · _${metric.streamSuffix}`;
      copy.append(heading, detail);
      const state = document.createElement("span");
      state.className = app.outputs.has(metric.id) ? "metric-added" : "metric-chevron";
      state.textContent = app.outputs.has(metric.id) ? "ADDED" : "›";
      option.addEventListener("click", () => {
        app.selectedMetricId = metric.id;
        renderMetricOptions();
        renderMetricDetail();
      });
      option.append(mark, copy, state);
      return option;
    });
    if (!options.length) {
      const empty = document.createElement("p");
      empty.className = "metric-library-empty";
      empty.textContent = "No metrics match this filter.";
      elements["metric-options"].replaceChildren(empty);
    } else {
      elements["metric-options"].replaceChildren(...options);
    }
    elements["metric-library-summary"].textContent = `${visible.length} of ${app.catalog.length} metrics`;
    renderMetricDetail();
  }

  function renderMetricDetail() {
    const metric = app.catalog.find((candidate) => candidate.id === app.selectedMetricId);
    const save = elements["save-metric-output"];
    const status = elements["dialog-output-status"];
    if (!metric) {
      const empty = document.createElement("div");
      empty.className = "metric-detail-empty";
      const kicker = document.createElement("span");
      kicker.textContent = "SELECT A METRIC";
      const title = document.createElement("strong");
      title.textContent = "Scientific context appears here";
      const copy = document.createElement("p");
      copy.textContent = "Nothing is added until you review a metric and press Save output.";
      empty.append(kicker, title, copy);
      elements["metric-detail"].replaceChildren(empty);
      save.disabled = true;
      save.textContent = "Save output";
      status.textContent = "Select one metric to inspect";
      return;
    }

    const article = document.createElement("article");
    const header = document.createElement("header");
    const category = document.createElement("p");
    category.className = "metric-detail-category";
    category.textContent = metric.category;
    const title = document.createElement("h3");
    title.textContent = metric.label;
    const evidence = document.createElement("span");
    evidence.className = "evidence-badge";
    evidence.textContent = metric.evidence || "research metric";
    header.append(category, title, evidence);

    const measurement = document.createElement("section");
    const measurementTitle = document.createElement("h4");
    measurementTitle.textContent = "What this output measures";
    const measurementCopy = document.createElement("p");
    measurementCopy.textContent = metric.detail;
    measurement.append(measurementTitle, measurementCopy);

    const consensus = document.createElement("section");
    const consensusTitle = document.createElement("h4");
    consensusTitle.textContent = "Current scientific view";
    const consensusCopy = document.createElement("p");
    consensusCopy.textContent = metric.explainer;
    consensus.append(consensusTitle, consensusCopy);

    const source = document.createElement("section");
    source.className = "metric-source";
    const sourceTitle = document.createElement("h4");
    sourceTitle.textContent = "Research source";
    const citation = document.createElement("a");
    citation.href = metric.citationUrl;
    citation.target = "_blank";
    citation.rel = "noreferrer";
    citation.textContent = `${metric.citationLabel} ↗`;
    source.append(sourceTitle, citation);

    const stream = document.createElement("section");
    stream.className = "metric-stream-preview";
    const streamTitle = document.createElement("h4");
    streamTitle.textContent = "Output created on save";
    const streamName = document.createElement("code");
    streamName.textContent = streamOutputName(metric, elements["stream-name"].value);
    const streamMeta = document.createElement("p");
    const transports = [elements["lsl-toggle"].checked ? "LSL" : null, elements["osc-toggle"].checked ? "OSC" : null].filter(Boolean);
    streamMeta.textContent = `${metric.channels} channel${metric.channels === 1 ? "" : "s"} · ${metric.unit} · ${transports.length ? transports.join(" + ") : "enable LSL or OSC in Output"}`;
    stream.append(streamTitle, streamName, streamMeta);

    article.append(header, measurement, consensus, source, stream);
    elements["metric-detail"].replaceChildren(article);
    const alreadyAdded = app.outputs.has(metric.id);
    save.disabled = alreadyAdded;
    save.textContent = alreadyAdded ? "Already added" : "Save output";
    status.textContent = alreadyAdded ? `${metric.label} is already in Output` : `Ready to add ${metric.label}`;
  }

  function renderOutputs() {
    const byId = new Map(app.catalog.map((metric) => [metric.id, metric]));
    const cards = [...app.outputs].map((id) => {
      const metric = byId.get(id);
      if (!metric) return null;
      const card = document.createElement("article");
      card.className = `output-card${metric.raw ? " raw-output-card" : ""}`;
      const header = document.createElement("header");
      const identity = document.createElement("span");
      const label = document.createElement("strong");
      label.textContent = metric.label;
      const stream = document.createElement("small");
      stream.textContent = streamOutputName(metric, elements["stream-name"].value);
      identity.append(label, stream);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.setAttribute("aria-label", `Remove ${metric.label}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        app.outputs.delete(id);
        delete app.metricOptions[id];
        renderOutputs();
        configureOutputs();
      });
      header.append(identity, remove);
      card.append(header);

      const controls = document.createElement("div");
      controls.className = "metric-controls";
      const options = metricOptionFor(id);
      const summary = document.createElement("span");
      summary.className = "module-summary";
      const scaling = options.normalization === "slidingWindow"
        ? `0–1 / ${options.windowSeconds}s`
        : options.normalization === "session" ? "0–1 / whole run" : "original scale";
      summary.textContent = `${options.displayWindowSeconds}s view · ${scaling}${id === "breathing_phase" ? ` · ${options.processing.breathingPhase.calibrationWindowSeconds}s calibration` : ""}`;
      const tune = document.createElement("button");
      tune.type = "button";
      tune.className = "module-tune-button";
      tune.textContent = "Adjust";
      tune.setAttribute("aria-label", `Adjust ${metric.label} module`);
      tune.addEventListener("click", () => openModuleSettings(id));
      controls.append(summary, tune);
      card.append(controls);
      return card;
    }).filter(Boolean);
    elements["output-chips"].replaceChildren(...cards);
    const count = app.outputs.size;
    elements["included-count"].textContent = `${count} active`;
    elements["output-state"].textContent = `${count} signal${count === 1 ? "" : "s"}`;
    updateStreamNamePreview();
    rebuildVisualOptions();
  }

  function metricOptionFor(id) {
    const stored = app.metricOptions[id] || {};
    const breathing = stored.processing?.breathingPhase || {};
    const options = {
      normalization: stored.normalization || "none",
      windowSeconds: Number(stored.windowSeconds) || 60,
      displayWindowSeconds: Number(stored.displayWindowSeconds) || 5,
      processing: {},
    };
    if (id === "breathing_phase") {
      options.processing.breathingPhase = {
        calibrationWindowSeconds: numberOr(breathing.calibrationWindowSeconds, 12),
        minimumAxisRangeG: numberOr(breathing.minimumAxisRangeG, 0.01),
        sampleEmaAlpha: numberOr(breathing.sampleEmaAlpha, 0.10),
        projectionEmaAlpha: numberOr(breathing.projectionEmaAlpha, 0.10),
        phaseDeltaThreshold: numberOr(breathing.phaseDeltaThreshold, 0.003),
        staleTimeoutSeconds: numberOr(breathing.staleTimeoutSeconds, 3),
        invertDirection: Boolean(breathing.invertDirection),
        adaptiveBounds: breathing.adaptiveBounds !== false,
        adaptiveWindowSeconds: numberOr(breathing.adaptiveWindowSeconds, 20),
        lowerQuantile: numberOr(breathing.lowerQuantile, 0.05),
        upperQuantile: numberOr(breathing.upperQuantile, 0.95),
      };
    }
    return options;
  }

  function numberOr(value, fallback) {
    const numeric = Number(value);
    return Number.isFinite(numeric) ? numeric : fallback;
  }

  function optionIdForVisual(id) {
    return visualDefinitions[id]?.parent || id;
  }

  function openModuleSettings(id) {
    const metric = app.catalog.find((candidate) => candidate.id === id);
    if (!metric || !app.outputs.has(id)) return;
    app.editingModuleId = id;
    app.moduleDraft = structuredClone(metricOptionFor(id));
    elements["module-dialog-title"].textContent = `Adjust ${metric.label}`;
    elements["module-dialog-intro"].textContent = id === "breathing_phase"
      ? "Tune the ACC classifier and its circle visualizer. Saving restarts breathing calibration; output changes are then applied to the matching LSL and OSC stream."
      : "Tune this visualizer and output transform. Changes remain a draft until Save module is pressed.";
    renderModuleSettings();
    elements["module-dialog"].showModal();
  }

  function renderModuleSettings() {
    const metric = app.catalog.find((candidate) => candidate.id === app.editingModuleId);
    if (!metric || !app.moduleDraft) return;
    const general = settingsSection("Visualizer and output", "The display window controls only the chart history. Normalization changes both the chart values and values published over LSL or OSC.");
    general.append(numberSetting("Display window", "Seconds visible in the low-latency visualizer.", app.moduleDraft.displayWindowSeconds, 1, 600, 1, (value) => {
      app.moduleDraft.displayWindowSeconds = clampNumber(value, 1, 600, 5);
    }));
    if (metric.normalizable) {
      general.append(selectSetting("Normalization", "Choose original units or a 0–1 transform.", app.moduleDraft.normalization, [
        ["none", "Original units"], ["slidingWindow", "0–1 sliding window"], ["session", "0–1 whole run"],
      ], (value) => {
        app.moduleDraft.normalization = value;
        renderModuleSettings();
      }));
      if (app.moduleDraft.normalization === "slidingWindow") {
        general.append(numberSetting("Normalization window", "Seconds used for running minimum and maximum.", app.moduleDraft.windowSeconds, 5, 3600, 5, (value) => {
          app.moduleDraft.windowSeconds = clampNumber(value, 5, 3600, 60);
        }));
      }
    } else {
      const note = document.createElement("div");
      note.className = "settings-note";
      note.textContent = metric.raw
        ? "Raw device streams stay in native units; only their visualizer history can be adjusted."
        : "This is a categorical or quality signal, so its published class values remain unscaled.";
      general.append(note);
    }

    const sections = [general];
    if (metric.id === "breathing_phase") {
      const classifier = app.moduleDraft.processing.breathingPhase;
      const processing = settingsSection("ACC breath-phase classifier", "These controls follow the original Polar tracker. The PCA axis is recalibrated whenever saved processing settings change.");
      processing.append(
        numberSetting("Calibration window", "Quiet chest-motion data used to learn the principal axis (seconds).", classifier.calibrationWindowSeconds, 1, 60, 1, (value) => classifier.calibrationWindowSeconds = clampNumber(value, 1, 60, 12)),
        numberSetting("Minimum motion range", "Required robust range on the learned axis (g).", classifier.minimumAxisRangeG, 0.001, 0.25, 0.001, (value) => classifier.minimumAxisRangeG = clampNumber(value, 0.001, 0.25, 0.01)),
        numberSetting("ACC smoothing", "EMA alpha; lower values smooth more strongly.", classifier.sampleEmaAlpha, 0.01, 1, 0.01, (value) => classifier.sampleEmaAlpha = clampNumber(value, 0.01, 1, 0.1)),
        numberSetting("Projection smoothing", "EMA alpha applied after PCA projection.", classifier.projectionEmaAlpha, 0.01, 1, 0.01, (value) => classifier.projectionEmaAlpha = clampNumber(value, 0.01, 1, 0.1)),
        numberSetting("Phase threshold", "Minimum 0–1 waveform change for inhale or exhale.", classifier.phaseDeltaThreshold, 0.0001, 0.25, 0.0001, (value) => classifier.phaseDeltaThreshold = clampNumber(value, 0.0001, 0.25, 0.003)),
        numberSetting("Stale timeout", "A longer data gap marks the next class as bad signal (seconds).", classifier.staleTimeoutSeconds, 0.25, 30, 0.25, (value) => classifier.staleTimeoutSeconds = clampNumber(value, 0.25, 30, 3)),
        checkSetting("Invert inhale / exhale", "Flips the learned movement axis when sensor orientation reverses the phases.", classifier.invertDirection, (value) => classifier.invertDirection = value),
        checkSetting("Adaptive normalization bounds", "Slowly follows posture and strap drift after calibration.", classifier.adaptiveBounds, (value) => {
          classifier.adaptiveBounds = value;
          renderModuleSettings();
        }),
      );
      if (classifier.adaptiveBounds) {
        processing.append(
          numberSetting("Adaptive window", "Recent projections used to refresh robust bounds (seconds).", classifier.adaptiveWindowSeconds, 5, 300, 1, (value) => classifier.adaptiveWindowSeconds = clampNumber(value, 5, 300, 20)),
          numberSetting("Lower quantile", "Robust lower projection bound.", classifier.lowerQuantile, 0, 0.4, 0.01, (value) => classifier.lowerQuantile = clampNumber(value, 0, 0.4, 0.05)),
          numberSetting("Upper quantile", "Robust upper projection bound.", classifier.upperQuantile, 0.6, 1, 0.01, (value) => classifier.upperQuantile = clampNumber(value, 0.6, 1, 0.95)),
        );
      }
      const warning = document.createElement("div");
      warning.className = "settings-note";
      warning.textContent = "Bad signal (−2) means calibration is incomplete or tracking became stale. It does not guarantee detection of every motion artifact; inspect the raw ACC and calibration output when signal quality matters.";
      processing.append(warning);
      sections.push(processing);
    }
    elements["module-settings"].replaceChildren(...sections);
    elements["module-dialog-status"].textContent = "Draft settings · press Save module to apply";
  }

  function settingsSection(titleText, descriptionText) {
    const section = document.createElement("section");
    section.className = "settings-section";
    const title = document.createElement("h3");
    title.textContent = titleText;
    const description = document.createElement("p");
    description.textContent = descriptionText;
    section.append(title, description);
    return section;
  }

  function numberSetting(labelText, helpText, value, min, max, step, onInput) {
    const field = settingField(labelText, helpText);
    const input = document.createElement("input");
    input.type = "number";
    input.min = String(min);
    input.max = String(max);
    input.step = String(step);
    input.value = String(value);
    input.addEventListener("input", () => onInput(input.value));
    field.insertBefore(input, field.lastChild);
    return field;
  }

  function selectSetting(labelText, helpText, value, choices, onChange) {
    const field = settingField(labelText, helpText);
    const select = document.createElement("select");
    for (const [choiceValue, choiceText] of choices) {
      const option = document.createElement("option");
      option.value = choiceValue;
      option.textContent = choiceText;
      select.append(option);
    }
    select.value = value;
    select.addEventListener("change", () => onChange(select.value));
    field.insertBefore(select, field.lastChild);
    return field;
  }

  function checkSetting(labelText, helpText, value, onChange) {
    const field = settingField(labelText, helpText);
    field.classList.add("setting-check");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = value;
    input.addEventListener("change", () => onChange(input.checked));
    field.prepend(input);
    return field;
  }

  function settingField(labelText, helpText) {
    const field = document.createElement("label");
    field.className = "setting-field";
    const label = document.createElement("span");
    label.textContent = labelText;
    const help = document.createElement("small");
    help.textContent = helpText;
    field.append(label, help);
    return field;
  }

  function clampNumber(value, minimum, maximum, fallback) {
    const number = Number(value);
    return Math.max(minimum, Math.min(maximum, Number.isFinite(number) ? number : fallback));
  }

  async function saveModuleSettings() {
    const id = app.editingModuleId;
    const metric = app.catalog.find((candidate) => candidate.id === id);
    if (!id || !metric || !app.moduleDraft) return;
    const draft = structuredClone(app.moduleDraft);
    if (!metric.normalizable) draft.normalization = "none";
    app.metricOptions[id] = draft;
    resetVisualTransform(id);
    for (const [visualId, definition] of Object.entries(visualDefinitions)) {
      if (definition.parent === id) resetVisualTransform(visualId);
    }
    renderOutputs();
    updateVisualLabels();
    await configureOutputs();
    elements["module-dialog"].close();
    toast(`${metric.label} settings saved${id === "breathing_phase" ? " · calibration restarted" : ""}`);
  }

  function resetVisualTransform(id) {
    delete app.visualNormalizers[id];
    ensureBuffer(id).clear();
  }

  function resetMeasurementVisuals() {
    app.visualNormalizers = {};
    for (const buffer of Object.values(buffers)) buffer.clear();
    app.sampleCount = 0;
    updateSampleCounter();
  }

  function visualValue(id, input) {
    const value = Number(input);
    if (!Number.isFinite(value)) return value;
    const options = metricOptionFor(optionIdForVisual(id));
    if (!options || options.normalization === "none") return value;
    const modeKey = `${options.normalization}:${options.windowSeconds || 60}`;
    let state = app.visualNormalizers[id];
    if (!state || state.modeKey !== modeKey) {
      state = { modeKey, minimum: Infinity, maximum: -Infinity, sequence: 0, minQueue: [], maxQueue: [], minHead: 0, maxHead: 0 };
      app.visualNormalizers[id] = state;
    }
    if (options.normalization === "session") {
      state.minimum = Math.min(state.minimum, value);
      state.maximum = Math.max(state.maximum, value);
      return minMaxValue(value, state.minimum, state.maximum);
    }
    const now = performance.now();
    state.sequence += 1;
    while (state.minQueue.length > state.minHead && state.minQueue.at(-1).value >= value) state.minQueue.pop();
    while (state.maxQueue.length > state.maxHead && state.maxQueue.at(-1).value <= value) state.maxQueue.pop();
    state.minQueue.push({ at: now, value });
    state.maxQueue.push({ at: now, value });
    const cutoff = now - Math.max(5, Number(options.windowSeconds) || 60) * 1000;
    while (state.minQueue[state.minHead]?.at < cutoff) state.minHead += 1;
    while (state.maxQueue[state.maxHead]?.at < cutoff) state.maxHead += 1;
    if (state.minHead > 1024) { state.minQueue = state.minQueue.slice(state.minHead); state.minHead = 0; }
    if (state.maxHead > 1024) { state.maxQueue = state.maxQueue.slice(state.maxHead); state.maxHead = 0; }
    return minMaxValue(value, state.minQueue[state.minHead]?.value ?? value, state.maxQueue[state.maxHead]?.value ?? value);
  }

  function minMaxValue(value, minimum, maximum) {
    return Math.abs(maximum - minimum) < Number.EPSILON ? 0.5 : Math.max(0, Math.min(1, (value - minimum) / (maximum - minimum)));
  }

  function updateStreamNamePreview() {
    const byId = new Map(app.catalog.map((metric) => [metric.id, metric]));
    const names = [...app.outputs]
      .map((id) => byId.get(id))
      .filter(Boolean)
      .map((metric) => streamOutputName(metric, elements["stream-name"].value));
    if (!normalizeStreamBase(elements["stream-name"].value)) {
      elements["stream-name-preview"].textContent = "Use at least one letter or number; spaces become underscores.";
      return;
    }
    const extra = names.length > 2 ? ` · +${names.length - 2} more` : "";
    elements["stream-name-preview"].textContent = names.length
      ? `Publishes ${names.slice(0, 2).join(" · ")}${extra}`
      : "No outputs are currently selected.";
  }

  function rebuildVisualOptions() {
    const choices = [];
    for (const [id, definition] of Object.entries(visualDefinitions)) {
      const parent = definition.parent || id;
      if (!app.outputs.has(parent)) continue;
      choices.push({ id, definition });
    }
    elements["visual-source"].replaceChildren(...choices.map(({ id, definition }) => {
      const option = document.createElement("option");
      option.value = id;
      option.textContent = definition.label;
      return option;
    }));
    if (!choices.some((choice) => choice.id === app.selectedVisual)) {
      app.selectedVisual = choices[0]?.id || "";
    }
    elements["visual-source"].value = app.selectedVisual;
    elements["visual-source"].disabled = !choices.length;
    updateVisualLabels();
  }

  function updateVisualLabels() {
    const definition = visualDefinitions[app.selectedVisual];
    const options = metricOptionFor(optionIdForVisual(app.selectedVisual));
    const normalized = options.normalization !== "none";
    elements["visual-label"].textContent = definition?.label || "No output selected";
    elements["visual-unit"].textContent = app.selectedVisual === "breathing_phase" ? "" : normalized ? "0–1" : definition?.unit || "";
    elements["visual-window-label"].textContent = app.selectedVisual === "breathing_phase" ? "Live phase" : `${options.displayWindowSeconds} second window`;
    elements["visual-scale-label"].textContent = normalized
      ? options.normalization === "session" ? "0–1 whole run" : `0–1 / ${options.windowSeconds}s`
      : "Original scale";
    elements["adjust-visual"].disabled = !definition;
    elements["chart-shell"].classList.toggle("phase-visual", app.selectedVisual === "breathing_phase");
    document.querySelector(".legend-line").style.background = definition?.color || "#87958d";
  }

  async function configureOutputs({ quiet = false } = {}) {
    const streamName = normalizeStreamBase(elements["stream-name"].value);
    if (!streamName) {
      elements["stream-name"].setAttribute("aria-invalid", "true");
      updateStreamNamePreview();
      return;
    }
    elements["stream-name"].removeAttribute("aria-invalid");
    app.streamName = streamName;
    const config = {
      streamName,
      lslEnabled: elements["lsl-toggle"].checked,
      oscEnabled: elements["osc-toggle"].checked,
      outputs: [...app.outputs],
      metricOptions: Object.fromEntries([...app.outputs].map((id) => [id, metricOptionFor(id)])),
    };
    if (!isNative) {
      elements["stream-name"].value = streamName;
      app.preferences = preferences.saveOutputConfig(config);
      renderOutputs();
      elements["lsl-detail"].textContent = config.lslEnabled ? "Preview · liblsl is checked in the native app" : "Local network · time synchronized";
      elements["osc-detail"].textContent = config.oscEnabled ? "Preview · UDP localhost:9000" : "UDP · localhost:9000";
      return;
    }

    const sequence = ++app.outputSequence;
    try {
      const health = await invoke("update_output_config", { config });
      if (sequence !== app.outputSequence) return;
      app.streamName = health.streamName || streamName;
      elements["stream-name"].value = app.streamName;
      config.streamName = app.streamName;
      app.preferences = preferences.saveOutputConfig(config);
      renderOutputs();
      updateDestinationHealth(health);
    } catch (error) {
      if (!quiet) toast(String(error), true);
    }
  }

  function updateDestinationHealth(health) {
    const lslText = elements["lsl-toggle"].checked ? health.lsl : "Local network · time synchronized";
    const oscText = elements["osc-toggle"].checked ? health.osc : "UDP · localhost:9000";
    elements["lsl-detail"].textContent = lslText;
    elements["osc-detail"].textContent = oscText;
    elements["lsl-detail"].classList.toggle("warning", elements["lsl-toggle"].checked && /not found|failed|could not|unavailable/i.test(lslText));
    elements["osc-detail"].classList.toggle("warning", elements["osc-toggle"].checked && /failed|could not|unavailable/i.test(oscText));
  }

  function resizeCanvas() {
    const canvas = elements["signal-canvas"];
    const bounds = elements["chart-shell"].getBoundingClientRect();
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.max(1, Math.round(bounds.width * ratio));
    canvas.height = Math.max(1, Math.round(bounds.height * ratio));
  }

  let previousFrame = performance.now();
  let frameAccumulator = 0;
  let frameSamples = 0;
  function drawFrame(now) {
    const elapsed = now - previousFrame;
    previousFrame = now;
    frameAccumulator += elapsed;
    frameSamples += 1;
    if (frameAccumulator >= 800) {
      elements["render-rate"].textContent = `${Math.round((frameSamples * 1000) / frameAccumulator)} fps`;
      frameAccumulator = 0;
      frameSamples = 0;
    }

    drawSignal();
    window.requestAnimationFrame(drawFrame);
  }

  function drawSignal() {
    const canvas = elements["signal-canvas"];
    const context = canvas.getContext("2d", { alpha: true });
    const definition = visualDefinitions[app.selectedVisual];
    const buffer = buffers[app.selectedVisual];
    if (!definition || !buffer) {
      context.clearRect(0, 0, canvas.width, canvas.height);
      elements["chart-empty"].hidden = false;
      elements["visual-current"].textContent = "—";
      return;
    }

    const options = metricOptionFor(optionIdForVisual(app.selectedVisual));
    if (app.selectedVisual === "breathing_phase") {
      drawBreathingPhase(context, canvas, buffer);
      return;
    }

    const visibleCount = Math.max(10, Math.ceil(definition.rate * options.displayWindowSeconds));
    const values = buffer.tail(visibleCount);
    const normalized = options.normalization !== "none";
    elements["chart-empty"].hidden = values.length > 1;
    elements["visual-current"].textContent = formatValue(buffer.latest(), normalized ? 3 : definition.unit === "g" ? 3 : definition.unit === "bpm" ? 0 : 1);
    if (values.length < 2) {
      context.clearRect(0, 0, canvas.width, canvas.height);
      return;
    }

    let min = Infinity;
    let max = -Infinity;
    for (const value of values) {
      if (value < min) min = value;
      if (value > max) max = value;
    }
    if (normalized) {
      min = 0;
      max = 1;
    } else if (definition.symmetric) {
      const extent = Math.max(Math.abs(min), Math.abs(max), 1) * 1.08;
      min = -extent;
      max = extent;
    } else {
      const padding = Math.max((max - min) * 0.12, Math.abs(max) * 0.02, 0.5);
      min -= padding;
      max += padding;
    }

    elements["y-max"].textContent = shortAxis(max);
    elements["y-min"].textContent = shortAxis(min);
    const width = canvas.width;
    const height = canvas.height;
    const padX = Math.round(width * 0.035);
    const padY = Math.round(height * 0.08);
    const drawWidth = width - padX * 2;
    const drawHeight = height - padY * 2;
    const range = max - min || 1;
    context.clearRect(0, 0, width, height);
    context.beginPath();
    for (let index = 0; index < values.length; index += 1) {
      const x = padX + (index / (values.length - 1)) * drawWidth;
      const y = padY + (1 - (values[index] - min) / range) * drawHeight;
      if (index === 0) context.moveTo(x, y); else context.lineTo(x, y);
    }
    context.strokeStyle = definition.color;
    context.lineWidth = Math.max(1.4, (window.devicePixelRatio || 1) * 0.9);
    context.lineJoin = "round";
    context.lineCap = "round";
    context.stroke();
  }

  function drawBreathingPhase(context, canvas, phaseBuffer) {
    const phase = phaseBuffer.latest();
    const width = canvas.width;
    const height = canvas.height;
    context.clearRect(0, 0, width, height);
    const descriptor = breathingPhaseDescriptor(phase);
    const hasPhase = phase != null;
    elements["chart-empty"].hidden = hasPhase;
    elements["visual-current"].textContent = hasPhase ? descriptor.label : "—";
    elements["y-max"].textContent = "";
    elements["y-min"].textContent = "";
    if (!hasPhase) return;

    const volume = Math.max(0, Math.min(1, Number(buffers.breathing_volume?.latest()) || 0.5));
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    const radius = Math.min(width, height) * (descriptor.bad ? 0.20 : 0.14 + volume * 0.20);
    const centerX = width / 2;
    const centerY = height / 2;
    const glow = context.createRadialGradient(centerX, centerY, radius * 0.25, centerX, centerY, radius * 1.35);
    glow.addColorStop(0, `${descriptor.color}d9`);
    glow.addColorStop(0.68, `${descriptor.color}8c`);
    glow.addColorStop(1, `${descriptor.color}00`);
    context.beginPath();
    context.arc(centerX, centerY, radius * 1.35, 0, Math.PI * 2);
    context.fillStyle = glow;
    context.fill();
    context.beginPath();
    context.arc(centerX, centerY, radius, 0, Math.PI * 2);
    context.fillStyle = descriptor.textColor;
    context.globalAlpha = descriptor.bad ? 0.18 : 0.22;
    context.fill();
    context.globalAlpha = 1;
    context.strokeStyle = descriptor.color;
    context.lineWidth = Math.max(2, pixelRatio * 2);
    if (descriptor.bad) context.setLineDash([8 * pixelRatio, 7 * pixelRatio]);
    context.stroke();
    context.setLineDash([]);
    context.fillStyle = descriptor.color;
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.font = `800 ${Math.max(13, Math.min(width, height) * 0.045)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
    context.fillText(descriptor.label, centerX, centerY);
    context.font = `600 ${Math.max(9, Math.min(width, height) * 0.026)}px system-ui, sans-serif`;
    context.globalAlpha = 0.72;
    context.fillText(descriptor.hint, centerX, centerY + Math.max(22, radius * 0.27));
    context.globalAlpha = 1;
  }

  function breathingPhaseDescriptor(value) {
    if (value <= -1.5) return { label: "BAD SIGNAL", hint: "calibrate or inspect ACC", color: "#b94b40", textColor: "#6f211b", bad: true };
    if (value > 0.5) return { label: "INHALE", hint: "chest projection rising", color: "#168259", textColor: "#083d29", bad: false };
    if (value < -0.5) return { label: "EXHALE", hint: "chest projection falling", color: "#d17a28", textColor: "#673605", bad: false };
    return { label: "PAUSE", hint: "change below threshold", color: "#3b78aa", textColor: "#173f62", bad: false };
  }

  function updateSparkline() {
    const values = buffers.raw_ecg.tail(56);
    if (values.length < 2) return;
    let min = Infinity;
    let max = -Infinity;
    for (const value of values) { min = Math.min(min, value); max = Math.max(max, value); }
    const range = max - min || 1;
    const path = [];
    for (let index = 0; index < values.length; index += 1) {
      const x = (index / (values.length - 1)) * 280;
      const y = 4 + (1 - (values[index] - min) / range) * 36;
      path.push(`${index ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`);
    }
    elements["ecg-spark"].setAttribute("d", path.join(" "));
  }

  function startDemoSignal() {
    stopDemoSignal();
    let lastMetric = 0;
    app.demoTimer = window.setInterval(() => {
      const ecg = [];
      const acc = [];
      for (let index = 0; index < 7; index += 1) {
        const phase = app.demoPhase + index / 130;
        const beatPhase = (phase * 1.18) % 1;
        const qrs = 880 * Math.exp(-Math.pow((beatPhase - 0.12) / 0.028, 2));
        const q = -180 * Math.exp(-Math.pow((beatPhase - 0.09) / 0.018, 2));
        const t = 150 * Math.exp(-Math.pow((beatPhase - 0.42) / 0.09, 2));
        ecg.push(Math.round(qrs + q + t + Math.sin(phase * 7) * 12));
      }
      for (let index = 0; index < 10; index += 1) {
        const phase = app.demoPhase + index / 200;
        acc.push({
          xMg: Math.round(Math.sin(phase * 4.5) * 85),
          yMg: Math.round(Math.cos(phase * 3.1) * 52),
          zMg: Math.round(995 + Math.sin(phase * 6.3) * 25),
        });
      }
      app.demoPhase += 0.05;
      handleNativeEvent({ kind: "ecg", sensorTimestampNs: 0, microvolts: ecg });
      handleNativeEvent({ kind: "accelerometer", sensorTimestampNs: 0, samples: acc });
      handleNativeEvent({ kind: "metrics", values: [
        { id: "breathing_calibration", value: 1 },
        { id: "breathing_volume", value: 0.5 + Math.sin(app.demoPhase * 1.25) * 0.42 },
        { id: "breathing_phase", value: Math.cos(app.demoPhase * 1.25) > 0.08 ? 1 : Math.cos(app.demoPhase * 1.25) < -0.08 ? -1 : 0 },
        { id: "breathing_axis_range", value: 0.034 },
      ] });
      if (app.demoPhase - lastMetric >= 1) {
        lastMetric = app.demoPhase;
        const values = app.catalog
          .filter((metric) => !metric.raw && !["acc_magnitude", "breathing_calibration", "breathing_volume", "breathing_phase", "breathing_axis_range"].includes(metric.id))
          .map((metric) => ({ id: metric.id, value: demoMetricValue(metric.id, app.demoPhase) }));
        handleNativeEvent({ kind: "metrics", values });
      }
    }, 50);
  }

  function demoMetricValue(id, phase) {
    const wave = Math.sin(phase * 0.7);
    if (id === "heart_rate" || id === "mean_heart_rate") return 71 + wave * 3;
    if (id === "rr_interval" || id === "mean_nn") return 60000 / (71 + wave * 3);
    if (id === "rmssd") return 31 + wave * 4;
    if (id === "ln_rmssd") return Math.log(31 + wave * 4);
    if (id === "sdnn") return 42 + wave * 5;
    if (id === "pnn50") return 18 + wave * 4;
    if (id === "sd1") return (31 + wave * 4) / Math.SQRT2;
    if (id === "coherence") return 0.62 + wave * 0.12;
    if (id === "coherence_confidence" || id === "breathing_dynamics_confidence") return 0.92;
    if (id === "heartmath_coherence") return 2.4 + wave * 0.5;
    if (id === "coherence_peak_frequency") return 0.1 + wave * 0.005;
    if (id.includes("coherence_") && id.includes("power")) return 1_200 + wave * 120;
    if (id === "breathing_rate") return 12 + wave;
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
    if (id === "ecg_peak_to_peak") return 1_050 + wave * 60;
    return 0.5 + wave * 0.1;
  }

  function stopDemoSignal() {
    if (app.demoTimer) window.clearInterval(app.demoTimer);
    app.demoTimer = null;
  }

  function formatValue(value, digits = 1) {
    const number = Number(value);
    if (!Number.isFinite(number)) return "—";
    return number.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
  }

  function shortAxis(value) {
    const absolute = Math.abs(value);
    if (absolute >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
    if (absolute < 10) return value.toFixed(1);
    return Math.round(value).toString();
  }

  async function renderInterfaceScenario(name) {
    const targets = {
      "breathing-phase-inhale": { phase: 1, volume: 0.82, label: "INHALE" },
      "breathing-phase-exhale": { phase: -1, volume: 0.18, label: "EXHALE" },
      "breathing-phase-pause": { phase: 0, volume: 0.50, label: "PAUSE" },
      "breathing-phase-bad-signal": { phase: -2, volume: 0.50, label: "BAD SIGNAL" },
    };
    const target = targets[name] || targets["breathing-phase-inhale"];
    app.outputs.add("breathing_phase");
    app.metricOptions.breathing_phase = structuredClone(metricOptionFor("breathing_phase"));
    renderOutputs();
    app.selectedVisual = "breathing_phase";
    elements["visual-source"].value = app.selectedVisual;
    updateVisualLabels();
    resizeCanvas();
    ensureBuffer("breathing_phase").clear();
    ensureBuffer("breathing_volume").clear();
    ensureBuffer("breathing_volume").push(target.volume);
    ensureBuffer("breathing_phase").push(target.phase);
    drawSignal();

    if (name === "breathing-phase-settings") {
      openModuleSettings("breathing_phase");
    } else if (elements["module-dialog"].open) {
      elements["module-dialog"].close();
    }
    await new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));
    resizeCanvas();
    drawSignal();
    return {
      scenario: name,
      expectedLabel: target.label,
      currentLabel: elements["visual-current"].textContent,
      selectedVisual: app.selectedVisual,
      streamName: streamOutputName(app.catalog.find((metric) => metric.id === "breathing_phase")),
      dialogOpen: elements["module-dialog"].open,
    };
  }

  const initialization = initialize();
  if (isInterfaceRenderer) {
    window.PolarInterfaceRenderer = Object.freeze({
      scenarios: Object.freeze([
        "breathing-phase-inhale",
        "breathing-phase-exhale",
        "breathing-phase-pause",
        "breathing-phase-bad-signal",
        "breathing-phase-settings",
      ]),
      ready: () => initialization,
      render: async (scenario) => {
        await initialization;
        return renderInterfaceScenario(scenario);
      },
      metricOptions: (id) => structuredClone(metricOptionFor(id)),
    });
  }
})();
