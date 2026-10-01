(() => {
  "use strict";

  const core = window.__TAURI__?.core;
  const nativeWindow = window.__TAURI__?.window?.getCurrentWindow?.();
  const isNative = Boolean(core?.invoke && core?.Channel);
  const directPolarOutputs = Object.freeze(["raw_ecg", "raw_acc", "heart_rate", "rr_interval"]);
  const releaseBreathingIds = Object.freeze([
    "breathing_volume",
    "breathing_signal_confidence",
    "breathing_signal_ready",
  ]);
  const state = {
    kind: "polar",
    productName: "Polar Stream Mini",
    scanLabel: "Polar H10",
    preferences: {
      streamName: "Polar-H10-Mini",
      outputMode: "separateStreams",
      autoConnect: true,
      polarOutputs: [...directPolarOutputs],
      lastDevice: null,
    },
    metrics: [],
    devices: [],
    selectedDeviceId: "",
    connected: false,
    streaming: false,
    mockMode: false,
    busy: false,
    status: "Initializing",
    lsl: "Off",
    samples: null,
    lastSampleAt: 0,
  };
  let savedPolarOutputs = [...directPolarOutputs];

  const elements = {};

  function bindElements() {
    for (const id of [
      "product-name",
      "theme-toggle",
      "new-node-top",
      "minimize-button",
      "close-button",
      "mini-node",
      "node-phase",
      "node-kind",
      "stream-name",
      "stream-mode-toggle",
      "autostart",
      "auto-connect",
      "device-row",
      "device-select",
      "scan-button",
      "mock-button",
      "mock-source",
      "metrics-button",
      "signal-list",
      "device-status",
      "lsl-status",
      "sample-status",
      "connect-button",
      "disconnect-button",
      "context-menu",
      "new-node-menu",
      "mock-node-menu",
      "metrics-dialog",
      "metrics-close",
      "metrics-guide",
      "metric-options",
      "reset-metrics",
    ]) {
      elements[id] = document.getElementById(id);
    }
    elements.polarOnly = [...document.querySelectorAll(".polar-only")];
    elements.hardwareOnly = [...document.querySelectorAll(".hardware-only")];
  }

  class RuntimeError extends Error {
    constructor(code, message, retryable = false) {
      super(message || "The native operation failed.");
      this.name = "RuntimeError";
      this.code = code || "NATIVE_OPERATION_FAILED";
      this.retryable = Boolean(retryable);
    }
  }

  function normalizeError(error) {
    if (error instanceof RuntimeError) return error;
    if (error && typeof error === "object") {
      return new RuntimeError(error.code, error.message || String(error), error.retryable);
    }
    return new RuntimeError("NATIVE_OPERATION_FAILED", String(error || "The native operation failed."));
  }

  async function invoke(command, payload = {}) {
    if (!isNative) {
      throw new RuntimeError("NATIVE_APP_REQUIRED", "Open the installed mini app to use Bluetooth and LSL.");
    }
    try {
      return await core.invoke(command, payload);
    } catch (error) {
      throw normalizeError(error);
    }
  }

  function preferencePayload() {
    return {
      streamName: elements["stream-name"].value.trim(),
      outputMode: elements["stream-mode-toggle"].checked ? "singleStream" : "separateStreams",
      autoConnect: elements["auto-connect"].checked,
      polarOutputs: [...new Set(state.preferences.polarOutputs || directPolarOutputs)],
    };
  }

  function setStatus(message, phase = null, attention = false) {
    state.status = message;
    elements["node-phase"].textContent = phase || message;
    elements["mini-node"].classList.toggle("attention", attention);
  }

  function renderBootstrap(bootstrap) {
    state.kind = bootstrap.kind || "polar";
    state.productName = bootstrap.productName || "Polar Stream Mini";
    state.scanLabel = bootstrap.scanLabel || "Polar H10";
    state.preferences = bootstrap.preferences || state.preferences;
    savedPolarOutputs = [...(state.preferences.polarOutputs || directPolarOutputs)];
    state.metrics = bootstrap.metrics || [];
    state.mockMode = Boolean(bootstrap.mockMode);
    state.connected = Boolean(bootstrap.session?.connected);
    state.streaming = false;
    state.lastSampleAt = 0;
    state.lsl = bootstrap.session?.lsl || "Off";
    const displayName = state.mockMode ? `${state.productName} Mock` : state.productName;
    document.title = displayName;
    document.body.dataset.kind = state.kind;
    document.body.dataset.mock = String(state.mockMode);
    elements["product-name"].textContent = displayName;
    elements["node-kind"].textContent = state.mockMode ? "MOCK" : state.kind === "polar" ? "POLAR" : "GDX";
    elements["stream-name"].value = state.preferences.streamName || "";
    elements["auto-connect"].checked = Boolean(state.preferences.autoConnect);
    elements["device-row"].hidden = state.mockMode;
    elements["mock-source"].hidden = !state.mockMode;
    elements["mock-button"].hidden = state.mockMode;
    elements.hardwareOnly.forEach((node) => {
      node.hidden = state.mockMode;
    });
    elements.polarOnly.forEach((node) => {
      node.hidden = state.kind !== "polar";
    });
    if (bootstrap.session?.deviceId) {
      state.selectedDeviceId = bootstrap.session.deviceId;
    }
    renderMode();
    renderDevices();
    renderSignals();
    renderMetricDialog();
    renderConnection(bootstrap.session);
    setStatus(isNative ? "Ready" : "Installed app required", null, !isNative);
  }

  function renderMode() {
    elements["stream-mode-toggle"].checked = state.preferences.outputMode === "singleStream";
  }

  function renderDevices() {
    const select = elements["device-select"];
    const previous = state.selectedDeviceId || select.value || state.preferences.lastDevice?.id;
    select.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = state.devices.length
      ? `Choose ${state.scanLabel}`
      : state.preferences.lastDevice
        ? `Saved: ${state.preferences.lastDevice.name}`
        : `Choose ${state.scanLabel}`;
    select.append(placeholder);
    for (const device of state.devices) {
      const option = document.createElement("option");
      option.value = device.id;
      option.textContent = device.rssi == null ? device.name : `${device.name} (${device.rssi} dBm)`;
      select.append(option);
    }
    const preferred = state.devices.find((device) => device.id === previous)
      || state.devices.find((device) => device.name.toLowerCase() === state.preferences.lastDevice?.name?.toLowerCase());
    if (preferred) {
      select.value = preferred.id;
      state.selectedDeviceId = preferred.id;
    } else {
      state.selectedDeviceId = "";
    }
  }

  function renderSignals() {
    const list = elements["signal-list"];
    list.replaceChildren();
    const chips = [];
    if (state.kind === "polar") {
      if (state.preferences.outputMode === "singleStream") {
        chips.push("single sparse LSL");
      }
      const selected = new Set(state.preferences.polarOutputs || directPolarOutputs);
      for (const id of directPolarOutputs) chips.push(streamSuffix(id));
      const optional = state.metrics.filter((metric) => !metric.direct && selected.has(metric.id));
      if (optional.length) chips.push(optional[0].streamSuffix || optional[0].id);
      if (optional.length > 1) chips.push(`+${optional.length - 1} metrics`);
      const extraCount = optional.length;
      elements["metrics-button"].title = extraCount
        ? `${extraCount} optional realtime metrics selected`
        : "Add optional realtime metrics";
    } else if (state.preferences.outputMode === "singleStream") {
      chips.push("single sparse LSL", "raw channels", "breathing");
    } else {
      chips.push("rawVernier", "vernierBreathing", "rawForce");
    }
    for (const chip of chips) {
      const span = document.createElement("span");
      span.textContent = chip;
      list.append(span);
    }
  }

  function streamSuffix(id) {
    return state.metrics.find((metric) => metric.id === id)?.streamSuffix || id;
  }

  function renderConnection(session = null) {
    elements["mini-node"].classList.toggle("connected", state.connected);
    elements["mini-node"].classList.toggle(
      "streaming",
      state.connected && state.streaming && isLslPublishing(state.lsl),
    );
    elements["connect-button"].disabled = state.busy || state.connected || !isNative;
    elements["connect-button"].textContent = state.mockMode ? "Start mock" : "Connect";
    elements["disconnect-button"].disabled = state.busy || !state.connected || !isNative;
    elements["disconnect-button"].textContent = state.mockMode ? "Stop mock" : "Disconnect";
    elements["scan-button"].disabled = state.busy || !isNative;
    elements["mock-button"].disabled = state.busy || !isNative;
    elements["device-select"].disabled = state.busy || state.connected || !isNative;
    elements["metrics-button"].disabled = state.busy || !isNative;
    elements["device-status"].textContent = state.connected
      ? session?.deviceName || state.preferences.lastDevice?.name || "Connected"
      : state.mockMode
        ? "Synthetic source"
      : state.preferences.lastDevice?.name
        ? `Saved ${state.preferences.lastDevice.name}`
        : "No device";
    elements["lsl-status"].textContent = state.lsl || "Off";
    elements["sample-status"].textContent = sampleSummary();
  }

  function sampleSummary() {
    const samples = state.samples;
    if (!samples) return "0";
    if (state.kind === "polar") {
      return `${samples.ecgSamples || 0} ECG, ${samples.accSamples || 0} ACC, ${samples.heartRatePackets || 0} HR`;
    }
    return `${samples.vernierRows || 0} rows, ${samples.metricSamples || 0} breathing`;
  }

  function isLslPublishing(status) {
    return String(status || "").startsWith("Publishing ");
  }

  function hasNewSamples(samples, previous) {
    return ["ecgSamples", "accSamples", "heartRatePackets", "metricSamples", "vernierRows"]
      .some((key) => Number(samples?.[key] || 0) > Number(previous?.[key] || 0));
  }

  function renderMetricDialog() {
    const options = elements["metric-options"];
    options.replaceChildren();
    if (state.kind !== "polar") return;
    const selected = new Set(state.preferences.polarOutputs || directPolarOutputs);
    elements["reset-metrics"].disabled = ![...selected].some((id) => !directPolarOutputs.includes(id));
    const optionalMetrics = state.metrics.filter((candidate) => !candidate.direct);
    appendMetricGroup(
      options,
      "ECG / heart derived",
      "Computed from ECG, heart rate, or RR intervals.",
      optionalMetrics.filter((metric) => !isAccDerivedMetric(metric)),
      selected,
    );
    appendMetricGroup(
      options,
      "ACC derived",
      "Computed from the H10's 200 Hz chest acceleration.",
      optionalMetrics.filter(isAccDerivedMetric),
      selected,
    );
  }

  function isAccDerivedMetric(metric) {
    return metric.id === "acc_magnitude"
      || metric.category === "Breathing"
      || metric.category === "Breathing dynamics";
  }

  function appendMetricGroup(options, titleText, summaryText, metrics, selected) {
    if (!metrics.length) return;
    const section = document.createElement("section");
    section.className = "metric-group";
    const heading = document.createElement("header");
    const title = document.createElement("strong");
    title.textContent = titleText;
    const summary = document.createElement("span");
    summary.textContent = summaryText;
    heading.append(title, summary);
    const list = document.createElement("div");
    list.className = "metric-group-list";
    for (const metric of metrics) {
      const label = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = metric.id;
      checkbox.checked = selected.has(metric.id);
      checkbox.addEventListener("change", () => {
        updateMetricSelection(metric.id, checkbox.checked);
        renderMetricDialog();
        renderSignals();
        savePreferences(false);
      });
      const copy = document.createElement("span");
      copy.className = "metric-copy";
      const title = document.createElement("strong");
      title.textContent = metric.label;
      const identity = document.createElement("code");
      identity.textContent = metric.id;
      const detail = document.createElement("span");
      detail.textContent = metric.detail || `${metric.category} / ${metric.unit || "value"}`;
      copy.append(title, identity, detail);
      label.append(checkbox, copy);
      list.append(label);
    }
    section.append(heading, list);
    options.append(section);
  }

  function updateMetricSelection(id, checked) {
    const selected = new Set(state.preferences.polarOutputs || directPolarOutputs);
    if (releaseBreathingIds.includes(id)) {
      for (const breathingId of releaseBreathingIds) {
        if (checked) selected.add(breathingId);
        else selected.delete(breathingId);
      }
    } else if (checked) {
      selected.add(id);
    } else {
      selected.delete(id);
    }
    for (const direct of directPolarOutputs) selected.add(direct);
    state.preferences.polarOutputs = [...selected];
  }

  let saveTimer = 0;
  let saveQueue = Promise.resolve();
  let saveRevision = 0;
  function scheduleSave() {
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => {
      savePreferences(true);
    }, 280);
  }

  async function savePreferences(quiet = false) {
    if (!isNative) return;
    const payload = preferencePayload();
    if (!payload.streamName) {
      setStatus("Stream name required", "Config", true);
      return;
    }
    const revision = ++saveRevision;
    const save = async () => {
      try {
        const result = await invoke("save_preferences", { preferences: payload });
        savedPolarOutputs = [...(result.preferences?.polarOutputs || payload.polarOutputs)];
        if (revision === saveRevision) {
          state.preferences = result.preferences || state.preferences;
          renderMode();
          renderSignals();
          if (!quiet || result.reconnectRequired || result.applied) {
            setStatus(result.message || "Saved", "Config", result.reconnectRequired);
          }
        }
      } catch (error) {
        if (revision === saveRevision) {
          state.preferences.polarOutputs = [...savedPolarOutputs];
          renderMetricDialog();
          renderMode();
          renderSignals();
          reportError(error);
        }
      }
    };
    saveQueue = saveQueue.then(save, save);
    return saveQueue;
  }

  async function scanDevices() {
    withBusy(async () => {
      setStatus(`Scanning for ${state.scanLabel}`, "Scan");
      state.devices = await invoke("scan_devices");
      renderDevices();
      setStatus(state.devices.length ? `Found ${state.devices.length}` : "No devices found", "Scan", !state.devices.length);
    });
  }

  async function connectSelected() {
    if (state.mockMode) {
      startMockStream();
      return;
    }
    withBusy(async () => {
      let deviceId = elements["device-select"].value;
      if (!deviceId) {
        state.devices = await invoke("scan_devices");
        renderDevices();
        deviceId = state.devices[0]?.id || "";
      }
      if (!deviceId) {
        throw new RuntimeError("NO_DEVICE_SELECTED", `No ${state.scanLabel} was selected.`, true);
      }
      setStatus("Connecting", "Connect");
      const session = await invoke("connect_device", {
        deviceId,
        preferences: preferencePayload(),
      });
      state.connected = Boolean(session.connected) || state.connected;
      state.selectedDeviceId = deviceId;
      state.lsl = session.lsl;
      renderConnection(session);
      setStatus(state.connected ? "Streaming" : "Looking for saved sensor", state.connected ? "Live" : "Reconnect");
    });
  }

  function startMockStream() {
    withBusy(async () => {
      setStatus("Starting synthetic source", "Mock");
      const session = await invoke("start_mock_stream", { preferences: preferencePayload() });
      state.connected = Boolean(session.connected) || state.connected;
      state.streaming = false;
      state.selectedDeviceId = session.deviceId;
      state.lsl = session.lsl;
      renderConnection(session);
      setStatus("Publishing synthetic data", "Live");
    });
  }

  async function connectRemembered() {
    if (!state.preferences.autoConnect || !state.preferences.lastDevice || state.connected) return;
    try {
      setStatus("Restoring saved sensor", "Scan");
      const session = await invoke("connect_remembered");
      state.connected = Boolean(session.connected) || state.connected;
      state.selectedDeviceId = session.deviceId;
      state.lsl = session.lsl;
      renderConnection(session);
      setStatus(state.connected ? "Streaming" : "Looking for saved sensor", state.connected ? "Live" : "Reconnect");
    } catch (error) {
      const normalized = normalizeError(error);
      setStatus(normalized.message, "Idle", normalized.retryable);
    }
  }

  async function syncAutostart() {
    if (!isNative) return;
    try {
      elements.autostart.checked = await invoke("get_autostart");
      elements.autostart.disabled = false;
    } catch (error) {
      elements.autostart.disabled = true;
      reportError(error);
    }
  }

  async function updateAutostart() {
    if (!isNative) return;
    const requested = elements.autostart.checked;
    elements.autostart.disabled = true;
    try {
      const enabled = await invoke("set_autostart", { enabled: requested });
      elements.autostart.checked = enabled;
      if (enabled !== requested) {
        throw new RuntimeError("AUTOSTART_NOT_APPLIED", "The operating system did not apply the startup preference.");
      }
      setStatus(enabled ? "Starts with PC" : "Startup disabled", "Config");
    } catch (error) {
      try {
        elements.autostart.checked = await invoke("get_autostart");
      } catch (_statusError) {
        elements.autostart.checked = !requested;
      }
      reportError(error);
    } finally {
      elements.autostart.disabled = false;
    }
  }

  async function openMetricsDialog() {
    renderMetricDialog();
    if (isNative) {
      try {
        await invoke("set_metrics_dialog_open", { open: true });
      } catch (_error) {
        // The compact fallback still exposes every metric when resizing is unavailable.
      }
    }
    elements["metrics-dialog"].showModal();
  }

  async function restoreCompactWindow() {
    if (!isNative) return;
    try {
      await invoke("set_metrics_dialog_open", { open: false });
    } catch (_error) {
      // Closing the picker must never block the app's normal controls.
    }
  }

  async function openMetricGuide() {
    try {
      await invoke("open_metrics_guide");
    } catch (error) {
      reportError(error);
    }
  }

  async function disconnect() {
    withBusy(async () => {
      setStatus("Disconnecting", "Stop");
      const session = await invoke("disconnect_device");
      state.connected = false;
      state.streaming = false;
      state.lastSampleAt = 0;
      state.lsl = "Off";
      state.samples = null;
      renderConnection(session);
      setStatus("Disconnected", "Idle");
    });
  }

  async function openNewNode() {
    try {
      const result = await invoke("open_new_node");
      setStatus(result.message || "Opened", "Node");
    } catch (error) {
      reportError(error);
    } finally {
      hideContextMenu();
    }
  }

  async function openMockNode() {
    try {
      const result = await invoke("open_mock_node");
      setStatus(result.message || "Mock opened", "Mock");
    } catch (error) {
      reportError(error);
    } finally {
      hideContextMenu();
    }
  }

  async function closeApp() {
    try {
      if (elements["stream-name"].value.trim()) {
        await savePreferences(true);
      }
      await invoke("close_app");
    } catch (_error) {
      window.close();
    }
  }

  async function minimizeApp() {
    if (!nativeWindow) return;
    try {
      await nativeWindow.minimize();
    } catch (error) {
      reportError(error);
    }
  }

  function withBusy(work) {
    state.busy = true;
    renderConnection();
    Promise.resolve()
      .then(work)
      .catch(reportError)
      .finally(() => {
        state.busy = false;
        renderConnection();
      });
  }

  function handleEvent(event) {
    if (!event || typeof event !== "object") return;
    if (event.kind === "status") {
      setStatus(event.message || "Status", event.phase || null);
    } else if (event.kind === "connection") {
      state.connected = Boolean(event.connected);
      state.streaming = false;
      state.lastSampleAt = 0;
      state.samples = null;
      if (!state.mockMode && event.connected && event.deviceName) {
        state.preferences.lastDevice = { id: state.selectedDeviceId, name: event.deviceName };
      }
      renderConnection({
        deviceName: event.deviceName,
        lsl: state.lsl,
      });
      setStatus(event.message || (event.connected ? "Streaming" : "Disconnected"), event.connected ? "Live" : "Idle");
    } else if (event.kind === "samples") {
      if (state.connected && isLslPublishing(event.lsl) && hasNewSamples(event, state.samples)) {
        state.lastSampleAt = performance.now();
      }
      state.samples = event;
      state.lsl = event.lsl || state.lsl;
      state.streaming = state.connected && isLslPublishing(state.lsl) && state.lastSampleAt > 0
        && performance.now() - state.lastSampleAt < 1200;
      renderConnection();
    } else if (event.kind === "error") {
      setStatus(event.message || "Warning", "Attention", true);
    }
  }

  function reportError(error) {
    const normalized = normalizeError(error);
    setStatus(normalized.message, "Attention", true);
  }

  function showContextMenu(event) {
    event.preventDefault();
    const menu = elements["context-menu"];
    const width = 184;
    menu.style.left = `${Math.min(event.clientX, window.innerWidth - width - 8)}px`;
    menu.style.top = `${Math.min(event.clientY, window.innerHeight - 44)}px`;
    menu.hidden = false;
  }

  function hideContextMenu() {
    elements["context-menu"].hidden = true;
  }

  function toggleTheme() {
    const current = document.documentElement.dataset.theme === "dark" ? "dark" : "light";
    const next = current === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    document.documentElement.style.colorScheme = next;
    try {
      window.localStorage.setItem(window.StreamMiniTheme.key, next);
    } catch (_error) {
      // Theme remains in memory for this launch.
    }
  }

  async function initNative() {
    if (!isNative) {
      renderBootstrap({ productName: "Polar Stream Mini", kind: "polar", scanLabel: "Polar H10", preferences: state.preferences });
      return;
    }
    const events = new core.Channel();
    events.onmessage = handleEvent;
    await invoke("attach_events", { events });
    const bootstrap = await invoke("get_bootstrap");
    renderBootstrap(bootstrap);
    if (state.mockMode) {
      window.setTimeout(startMockStream, 120);
    } else {
      await syncAutostart();
      window.setTimeout(connectRemembered, 250);
    }
  }

  function installHandlers() {
    elements["theme-toggle"].addEventListener("click", toggleTheme);
    elements["new-node-top"].addEventListener("click", openNewNode);
    elements["minimize-button"].addEventListener("click", minimizeApp);
    elements["close-button"].addEventListener("click", closeApp);
    elements["new-node-menu"].addEventListener("click", openNewNode);
    elements["mock-node-menu"].addEventListener("click", openMockNode);
    elements["mini-node"].addEventListener("contextmenu", showContextMenu);
    document.addEventListener("click", hideContextMenu);
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") hideContextMenu();
    });
    elements["stream-name"].addEventListener("input", scheduleSave);
    elements.autostart.addEventListener("change", updateAutostart);
    elements["auto-connect"].addEventListener("change", () => savePreferences(false));
    elements["device-select"].addEventListener("change", () => {
      state.selectedDeviceId = elements["device-select"].value;
    });
    elements["stream-mode-toggle"].addEventListener("change", () => {
      savePreferences(false);
    });
    elements["scan-button"].addEventListener("click", scanDevices);
    elements["mock-button"].addEventListener("click", openMockNode);
    elements["connect-button"].addEventListener("click", connectSelected);
    elements["disconnect-button"].addEventListener("click", disconnect);
    elements["metrics-button"].addEventListener("click", openMetricsDialog);
    elements["metrics-guide"].addEventListener("click", openMetricGuide);
    elements["metrics-dialog"].addEventListener("close", restoreCompactWindow);
    elements["reset-metrics"].addEventListener("click", () => {
      state.preferences.polarOutputs = [...directPolarOutputs];
      renderMetricDialog();
      renderSignals();
      savePreferences(false);
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    bindElements();
    installHandlers();
    window.setInterval(() => {
      if (state.streaming && performance.now() - state.lastSampleAt >= 1200) {
        state.streaming = false;
        renderConnection();
      }
    }, 400);
    initNative().catch(reportError);
  });
})();
