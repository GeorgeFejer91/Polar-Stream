(() => {
  "use strict";

  const nativeCore = window.__TAURI__?.core;
  const isNative = Boolean(nativeCore?.invoke && nativeCore?.Channel);
  const invoke = nativeCore?.invoke?.bind(nativeCore);
  const NativeChannel = nativeCore?.Channel;
  const preferences = window.PolarPreferences;

  const evidenceLinks = {
    hrv: ["Shaffer & Ginsberg (2017)", "https://www.frontiersin.org/journals/public-health/articles/10.3389/fpubh.2017.00258/full"],
    breathing: ["Drummond et al. (2021)", "https://consensus.app/papers/details/9389301a991e52ee9210f51939d318d7/?utm_source=unknown"],
    complexity: ["Bará et al. (2024)", "https://consensus.app/papers/details/de562a29bb8454eda201852b544fd9d7/?utm_source=unknown"],
    resonance: ["Sévoz-Couche & Laborde (2022)", "https://www.sciencedirect.com/science/article/abs/pii/S0149763422000653"],
    quality: ["Smital et al. (2020)", "https://consensus.app/papers/details/ad2a724fefd55d25baee823438fc672e/?utm_source=unknown"],
    stress: ["Immanuel et al. (2023)", "https://consensus.app/papers/details/14838ea9a9045710b4a676dbb7d595aa/?utm_source=unknown"],
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
    fallbackMetric("breathing_phase", "breathingPhase", "Inhale / exhale phase", "class", "Breathing", "breathing", "+1 inhale · −1 exhale · 0 pause", false, false, 20),
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
    fallbackMetric("excitometer", "excitometer", "Excitometer (experimental)", "0–1", "Excitation (experimental)", "stress", "Within-session HR ↑ plus lnRMSSD ↓"),
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
    app.outputs = new Set(bootstrap.config?.outputs || ["raw_ecg", "raw_acc"]);
    app.metricOptions = structuredClone(bootstrap.config?.metricOptions || {});
    installCatalogVisuals();
    app.streamName = normalizeStreamBase(app.preferences.streamName)
      || normalizeStreamBase(bootstrap.config?.streamName)
      || "Polar-H10";
    elements["stream-name"].value = app.streamName;
    elements["lsl-toggle"].checked = Boolean(bootstrap.config?.lslEnabled);
    elements["osc-toggle"].checked = Boolean(bootstrap.config?.oscEnabled);
    elements["platform-label"].textContent = String(bootstrap.platform || "local").toUpperCase();
    if (!isNative) elements["scan-caption"].textContent = "Interactive browser preview";

    renderMetricFilters();
    renderMetricOptions();
    renderOutputs();
    installInteractions();
    await configureOutputs({ quiet: true });
    resizeCanvas();
    window.requestAnimationFrame(drawFrame);
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
      if (metric.normalizable) {
        const options = metricOptionFor(id);
        const normalizeLabel = document.createElement("label");
        normalizeLabel.textContent = "Scale";
        const select = document.createElement("select");
        select.setAttribute("aria-label", `${metric.label} normalization`);
        [["none", "Original"], ["slidingWindow", "0–1 sliding"], ["session", "0–1 whole run"]].forEach(([value, text]) => {
          const option = document.createElement("option");
          option.value = value;
          option.textContent = text;
          select.append(option);
        });
        select.value = options.normalization || "none";
        select.addEventListener("change", () => {
          app.metricOptions[id] = { ...metricOptionFor(id), normalization: select.value };
          resetVisualTransform(id);
          renderOutputs();
          configureOutputs();
        });
        normalizeLabel.append(select);
        controls.append(normalizeLabel);
        if (select.value === "slidingWindow") {
          const windowLabel = document.createElement("label");
          windowLabel.textContent = "Window";
          const input = document.createElement("input");
          input.type = "number";
          input.min = "5";
          input.max = "3600";
          input.step = "5";
          input.value = String(options.windowSeconds || 60);
          input.setAttribute("aria-label", `${metric.label} normalization window in seconds`);
          input.addEventListener("change", () => {
            const seconds = Math.max(5, Math.min(3600, Number(input.value) || 60));
            input.value = String(seconds);
            app.metricOptions[id] = { ...metricOptionFor(id), windowSeconds: seconds };
            resetVisualTransform(id);
            configureOutputs();
          });
          const unit = document.createElement("span");
          unit.textContent = "s";
          windowLabel.append(input, unit);
          controls.append(windowLabel);
        }
      } else {
        const fixed = document.createElement("span");
        fixed.className = "fixed-output-note";
        fixed.textContent = metric.raw ? "Native samples" : "Categorical · scaling off";
        controls.append(fixed);
      }
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
    return app.metricOptions[id] || { normalization: "none", windowSeconds: 60 };
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
    const options = metricOptionFor(id);
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
    const normalized = metricOptionFor(app.selectedVisual).normalization !== "none";
    elements["visual-label"].textContent = definition?.label || "No output selected";
    elements["visual-unit"].textContent = normalized ? "0–1" : definition?.unit || "";
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
      app.preferences = preferences.saveStreamName(streamName);
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
      app.preferences = preferences.saveStreamName(app.streamName);
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

    const visibleCount = Math.max(10, Math.ceil(definition.rate * 5));
    const values = buffer.tail(visibleCount);
    const normalized = metricOptionFor(app.selectedVisual).normalization !== "none";
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

  initialize();
})();
