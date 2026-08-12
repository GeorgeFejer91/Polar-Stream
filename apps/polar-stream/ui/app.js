(() => {
  "use strict";

  const nativeCore = window.__TAURI__?.core;
  const isNative = Boolean(nativeCore?.invoke && nativeCore?.Channel);
  const invoke = nativeCore?.invoke?.bind(nativeCore);
  const NativeChannel = nativeCore?.Channel;
  const preferences = window.PolarPreferences;

  const fallbackCatalog = [
    { id: "raw_ecg", streamSuffix: "rawECG", label: "Raw ECG", detail: "130 Hz · 1 channel", unit: "µV", raw: true },
    { id: "raw_acc", streamSuffix: "rawACC", label: "Raw accelerometer", detail: "200 Hz · X, Y, Z", unit: "mg", raw: true },
    { id: "heart_rate", streamSuffix: "heartRate", label: "Heart rate", detail: "Device-derived", unit: "bpm", raw: false },
    { id: "rr_interval", streamSuffix: "rrInterval", label: "RR interval", detail: "Beat-to-beat interval", unit: "ms", raw: false },
    { id: "acc_magnitude", streamSuffix: "accMagnitude", label: "ACC magnitude", detail: "√(x² + y² + z²)", unit: "g", raw: false },
    { id: "rmssd", streamSuffix: "rmssd", label: "RMSSD", detail: "Rolling 60-beat window", unit: "ms", raw: false },
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
    "metric-options", "dialog-selection-count", "toast-region", "stream-name-preview",
  ];
  for (const id of ids) elements[id] = document.getElementById(id);

  const app = {
    connected: false,
    connecting: false,
    scanning: false,
    configuring: false,
    catalog: fallbackCatalog,
    outputs: new Set(["raw_ecg", "raw_acc"]),
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
      config: { streamName: "Polar-H10", lslEnabled: false, oscEnabled: false, outputs: ["raw_ecg", "raw_acc"] },
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
    app.streamName = normalizeStreamBase(app.preferences.streamName)
      || normalizeStreamBase(bootstrap.config?.streamName)
      || "Polar-H10";
    elements["stream-name"].value = app.streamName;
    elements["lsl-toggle"].checked = Boolean(bootstrap.config?.lslEnabled);
    elements["osc-toggle"].checked = Boolean(bootstrap.config?.oscEnabled);
    elements["platform-label"].textContent = String(bootstrap.platform || "local").toUpperCase();
    if (!isNative) elements["scan-caption"].textContent = "Interactive browser preview";

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
      syncDialogSelection();
      elements["output-dialog"].showModal();
    });
    elements["output-dialog"].addEventListener("close", () => {
      if (elements["output-dialog"].returnValue !== "confirm") return;
      const selected = elements["metric-options"].querySelectorAll("input:checked");
      app.outputs = new Set([...selected].map((input) => input.value));
      renderOutputs();
      configureOutputs();
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
      buffers.acc_magnitude.push(Math.hypot(x, y, z) / 1000);
    }
    const last = samples[samples.length - 1];
    elements["raw-acc-x"].textContent = formatValue(last.xMg ?? last.x_mg, 0);
    elements["raw-acc-y"].textContent = formatValue(last.yMg ?? last.y_mg, 0);
    elements["raw-acc-z"].textContent = formatValue(last.zMg ?? last.z_mg, 0);
    app.sampleCount += samples.length;
    updateSampleCounter();
  }

  function ingestMetrics(event) {
    buffers.heart_rate.push(event.heartRateBpm ?? event.heart_rate_bpm);
    buffers.rr_interval.pushMany(event.rrIntervalsMs ?? event.rr_intervals_ms ?? []);
    const rmssd = event.rmssdMs ?? event.rmssd_ms;
    if (rmssd != null) buffers.rmssd.push(rmssd);
  }

  function updateSampleCounter() {
    elements["sample-counter"].textContent = `${app.sampleCount.toLocaleString()} samples`;
  }

  function renderMetricOptions() {
    const options = app.catalog.map((metric) => {
      const label = document.createElement("label");
      label.className = "metric-option";
      const mark = document.createElement("span");
      mark.textContent = metric.raw ? "RAW" : "FX";
      const copy = document.createElement("span");
      const name = document.createElement("strong");
      name.textContent = metric.label;
      const detail = document.createElement("small");
      detail.textContent = `${metric.detail} · ${metric.unit} · _${metric.streamSuffix}`;
      copy.append(name, detail);
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = metric.id;
      checkbox.addEventListener("change", updateDialogCount);
      label.append(mark, copy, checkbox);
      return label;
    });
    elements["metric-options"].replaceChildren(...options);
  }

  function syncDialogSelection() {
    elements["metric-options"].querySelectorAll("input").forEach((input) => {
      input.checked = app.outputs.has(input.value);
    });
    updateDialogCount();
  }

  function updateDialogCount() {
    const count = elements["metric-options"].querySelectorAll("input:checked").length;
    elements["dialog-selection-count"].textContent = `${count} selected`;
  }

  function renderOutputs() {
    const byId = new Map(app.catalog.map((metric) => [metric.id, metric]));
    const chips = [...app.outputs].map((id) => {
      const metric = byId.get(id);
      if (!metric) return null;
      const chip = document.createElement("span");
      chip.className = "output-chip";
      const dot = document.createElement("i");
      const label = document.createElement("span");
      label.textContent = streamOutputName(metric, elements["stream-name"].value);
      chip.title = metric.label;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.setAttribute("aria-label", `Remove ${metric.label}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        app.outputs.delete(id);
        renderOutputs();
        configureOutputs();
      });
      chip.append(dot, label, remove);
      return chip;
    }).filter(Boolean);
    elements["output-chips"].replaceChildren(...chips);
    const count = app.outputs.size;
    elements["included-count"].textContent = `${count} active`;
    elements["output-state"].textContent = `${count} signal${count === 1 ? "" : "s"}`;
    updateStreamNamePreview();
    rebuildVisualOptions();
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
    elements["visual-label"].textContent = definition?.label || "No output selected";
    elements["visual-unit"].textContent = definition?.unit || "";
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
    elements["chart-empty"].hidden = values.length > 1;
    elements["visual-current"].textContent = formatValue(buffer.latest(), definition.unit === "g" ? 3 : definition.unit === "bpm" ? 0 : 1);
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
    if (definition.symmetric) {
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
      if (app.demoPhase - lastMetric >= 1) {
        lastMetric = app.demoPhase;
        handleNativeEvent({ kind: "metrics", heartRateBpm: 71, rrIntervalsMs: [845 + Math.sin(app.demoPhase) * 18], rmssdMs: 28.4 });
      }
    }, 50);
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
