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
  const vernierOutputs = Object.freeze([
    { id: "rawVernier", label: "Vernier channels", detail: "Force, respiration rate, steps, step rate, and packet diagnostics as received." },
    { id: "rawForce", label: "Force only", detail: "Belt tension in newtons, copied from the Vernier Force channel." },
    { id: "vernierBreathing", label: "Breathing waveform", detail: "Our live 0-1 normalization of belt force, not lung volume or breath rate." },
    { id: "signalStatus", label: "Signal status", detail: "Markers for lost and restored Bluetooth signal." },
  ]);
  const state = {
    kind: "vernier",
    productName: "Vernier Stream Mini",
    scanLabel: "Vernier Go Direct",
    preferences: {
      streamName: "Vernier-GDX-Mini",
      outputMode: "separateStreams",
      autoConnect: true,
      polarOutputs: [...directPolarOutputs],
      vernierOutputs: vernierOutputs.map((output) => output.id),
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
    radioStatus: "unknown",
    radioBusy: false,
  };

  const elements = {};
  let confirmedPreferences = null;

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
      "device-dialog",
      "device-dialog-close",
      "device-scan-status",
      "device-results",
      "device-rescan",
      "device-done",
      "bluetooth-toggle",
      "bluetooth-state",
      "device-error-details",
      "device-error-code",
      "mock-button",
      "mock-source",
      "metrics-button",
      "metric-count",
      "signal-list",
      "device-status",
      "lsl-status",
      "sample-status",
      "connection-feedback",
      "connect-button",
      "disconnect-button",
      "context-menu",
      "new-node-menu",
      "mock-node-menu",
      "metrics-dialog",
      "metrics-close",
      "metric-options",
      "apply-metrics",
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
      vernierOutputs: [...new Set(state.preferences.vernierOutputs || vernierOutputs.map((output) => output.id))],
    };
  }

  function setStatus(message, phase = null, attention = false) {
    state.status = message;
    elements["node-phase"].textContent = phase || message;
    elements["connection-feedback"].textContent = message;
    elements["mini-node"].classList.toggle("attention", attention);
  }

  function renderBootstrap(bootstrap) {
    state.kind = bootstrap.kind || "vernier";
    state.productName = bootstrap.productName || "Vernier Stream Mini";
    state.scanLabel = bootstrap.scanLabel || "Vernier Go Direct";
    state.preferences = bootstrap.preferences || state.preferences;
    confirmedPreferences = state.preferences;
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
    const remembered = state.preferences.recentDevices?.length
      ? state.preferences.recentDevices
      : state.preferences.lastDevice ? [state.preferences.lastDevice] : [];
    select.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = remembered.length ? "Previously connected" : "No saved devices";
    select.append(placeholder);
    for (const device of remembered) {
      const option = document.createElement("option");
      option.value = device.id;
      option.textContent = device.name;
      select.append(option);
    }
    const preferred = remembered.find((device) => device.id === previous)
      || remembered.find((device) => device.id === state.preferences.lastDevice?.id);
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
      for (const metric of state.metrics) {
        if (!metric.direct && selected.has(metric.id)) chips.push(metric.streamSuffix || metric.id);
      }
      const extraCount = state.metrics.filter((metric) => !metric.direct && selected.has(metric.id)).length;
      elements["metric-count"].textContent = extraCount ? `${extraCount} extra` : "Direct only";
    } else {
      const selected = new Set(state.preferences.vernierOutputs || vernierOutputs.map((output) => output.id));
      if (state.preferences.outputMode === "singleStream") chips.push("single sparse LSL");
      for (const output of vernierOutputs) {
        if (selected.has(output.id)) chips.push(output.id);
      }
      elements["metric-count"].textContent = `${selected.size}/4`;
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
    elements["bluetooth-toggle"].disabled = state.busy || state.radioBusy || state.connected || !["on", "off"].includes(state.radioStatus);
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
    if (state.kind === "vernier") {
      const selected = new Set(state.preferences.vernierOutputs || vernierOutputs.map((output) => output.id));
      for (const output of vernierOutputs) {
        const label = document.createElement("label");
        const checkbox = document.createElement("input");
        checkbox.type = "checkbox";
        checkbox.value = output.id;
        checkbox.checked = selected.has(output.id);
        checkbox.addEventListener("change", () => {
          const current = new Set(state.preferences.vernierOutputs || vernierOutputs.map((candidate) => candidate.id));
          if (!checkbox.checked && current.size === 1) {
            checkbox.checked = true;
            setStatus("Keep at least one output", "Config", true);
            return;
          }
          if (checkbox.checked) current.add(output.id);
          else current.delete(output.id);
          state.preferences.vernierOutputs = vernierOutputs
            .filter((candidate) => current.has(candidate.id)).map((candidate) => candidate.id);
          renderSignals();
          savePreferences(true);
        });
        const copy = document.createElement("span");
        const title = document.createElement("strong");
        title.textContent = output.label;
        const detail = document.createElement("span");
        detail.textContent = output.detail;
        copy.append(title, detail);
        label.append(checkbox, copy);
        options.append(label);
      }
      return;
    }
    const selected = new Set(state.preferences.polarOutputs || directPolarOutputs);
    for (const metric of state.metrics.filter((candidate) => !candidate.direct)) {
      const label = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = metric.id;
      checkbox.checked = selected.has(metric.id);
      checkbox.addEventListener("change", () => {
        updateMetricSelection(metric.id, checkbox.checked);
        renderMetricDialog();
        renderSignals();
      });
      const copy = document.createElement("span");
      const title = document.createElement("strong");
      title.textContent = metric.label;
      const detail = document.createElement("span");
      detail.textContent = `${metric.category} / ${metric.unit || "value"}`;
      copy.append(title, detail);
      label.append(checkbox, copy);
      options.append(label);
    }
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
        confirmedPreferences = result.preferences || state.preferences;
        if (revision === saveRevision) {
          state.preferences = confirmedPreferences;
          renderMode();
          renderSignals();
          if (elements["metrics-dialog"].open) renderMetricDialog();
          if (!quiet || result.reconnectRequired || result.applied) {
            setStatus(result.message || "Saved", "Config", result.reconnectRequired);
          }
        }
      } catch (error) {
        if (revision === saveRevision) {
          if (confirmedPreferences) {
            state.preferences = confirmedPreferences;
            elements["stream-name"].value = confirmedPreferences.streamName || "";
            elements["auto-connect"].checked = Boolean(confirmedPreferences.autoConnect);
          }
          renderMode();
          renderSignals();
          if (elements["metrics-dialog"].open) renderMetricDialog();
          reportError(error);
        }
      }
    };
    saveQueue = saveQueue.then(save, save);
    return saveQueue;
  }

  async function scanDevices() {
    if (!elements["device-dialog"].open) elements["device-dialog"].showModal();
    elements["device-results"].replaceChildren();
    elements["device-error-details"].hidden = true;
    elements["device-scan-status"].textContent = "Checking Bluetooth...";
    elements["device-rescan"].disabled = true;
    withBusy(async () => {
      try {
        const radio = await refreshRadio();
        if (radio?.state === "off") {
          const message = "Bluetooth is off. Switch it on to search for the belt.";
          elements["device-scan-status"].textContent = message;
          setStatus(message, "Bluetooth", true);
          return;
        }
        if (radio?.state === "disabled") {
          const message = "Bluetooth is blocked by hardware or Windows policy.";
          elements["device-scan-status"].textContent = message;
          setStatus(message, "Bluetooth", true);
          return;
        }
        setStatus(`Scanning for ${state.scanLabel}`, "Scan");
        elements["device-scan-status"].textContent = "Searching for nearby Go Direct sensors...";
        state.devices = await invoke("scan_devices");
        renderDiscoveredDevices();
        const message = state.devices.length
          ? `${state.devices.length} Go Direct device${state.devices.length === 1 ? "" : "s"} found. Select one to connect.`
          : "No Go Direct devices found. Wake the belt, keep it nearby, then rescan.";
        elements["device-scan-status"].textContent = message;
        setStatus(message, "Scan", !state.devices.length);
      } catch (error) {
        const normalized = normalizeError(error);
        const message = /0x800710df|device is not ready/i.test(normalized.message)
          ? "Windows Bluetooth is not ready. Check the radio, then rescan."
          : /bluetooth|adapter|radio|powered off|disabled|access denied/i.test(normalized.message)
            ? "Bluetooth is unavailable. Check Windows Bluetooth settings, then rescan."
            : "Device search failed. Check the belt, then rescan.";
        elements["device-scan-status"].textContent = message;
        elements["device-error-code"].textContent = normalized.message;
        elements["device-error-details"].hidden = false;
        setStatus(message, "Scan", true);
      } finally {
        elements["device-rescan"].disabled = false;
      }
    });
  }

  async function refreshRadio() {
    try {
      const result = await invoke("get_bluetooth_radio");
      state.radioStatus = result.state;
    } catch (_error) {
      state.radioStatus = "unknown";
    }
    const labels = {
      on: "On", off: "Off", disabled: "Blocked", unavailable: "No radio", unsupported: "System managed", unknown: "Unavailable",
    };
    elements["bluetooth-state"].textContent = labels[state.radioStatus] || "Unavailable";
    elements["bluetooth-toggle"].checked = state.radioStatus === "on";
    renderConnection();
    return { state: state.radioStatus };
  }

  async function changeRadio() {
    if (state.radioBusy) return;
    const enabled = elements["bluetooth-toggle"].checked;
    state.radioBusy = true;
    renderConnection();
    elements["device-scan-status"].textContent = enabled ? "Turning Bluetooth on..." : "Turning Bluetooth off...";
    try {
      await invoke("set_bluetooth_radio", { enabled });
      for (let attempt = 0; attempt < 6; attempt += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, 300));
        await refreshRadio();
        if (state.radioStatus === (enabled ? "on" : "off")) break;
      }
      if (state.radioStatus !== (enabled ? "on" : "off")) {
        throw new RuntimeError("RADIO_NOT_READY", "Windows accepted the request, but Bluetooth has not changed state.", true);
      }
      const message = enabled ? "Bluetooth is on. Searching for the belt..." : "Bluetooth is off.";
      elements["device-scan-status"].textContent = message;
      setStatus(message, "Bluetooth");
      if (enabled) window.setTimeout(scanDevices, 0);
    } catch (error) {
      const message = normalizeError(error).message;
      elements["device-scan-status"].textContent = message;
      setStatus(message, "Bluetooth", true);
      await refreshRadio();
    } finally {
      state.radioBusy = false;
      renderConnection();
    }
  }

  function renderDiscoveredDevices() {
    const list = elements["device-results"];
    list.replaceChildren();
    for (const device of state.devices) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "discovered-device";
      const name = document.createElement("strong");
      name.textContent = device.name;
      const detail = document.createElement("span");
      detail.textContent = [device.modelCode, device.rssi == null ? null : `${device.rssi} dBm`]
        .filter(Boolean).join(" / ");
      button.append(name, detail);
      button.addEventListener("click", () => {
        elements["device-dialog"].close();
        connectDevice(device.id);
      });
      list.append(button);
    }
  }

  function connectSelected() {
    if (state.mockMode) {
      startMockStream();
      return;
    }
    const deviceId = elements["device-select"].value;
    if (!deviceId) {
      scanDevices();
      return;
    }
    connectDevice(deviceId);
  }

  function connectDevice(deviceId) {
    withBusy(async () => {
      state.selectedDeviceId = deviceId;
      setStatus("Opening Bluetooth connection", "Connect");
      const session = await invoke("connect_device", {
        deviceId,
        preferences: preferencePayload(),
      });
      state.connected = Boolean(session.connected) || state.connected;
      state.selectedDeviceId = deviceId;
      state.lsl = session.lsl;
      renderConnection(session);
      if (!state.streaming) setStatus(state.connected ? "Connected; waiting for fresh LSL samples" : "Looking for saved sensor", state.connected ? "Connected" : "Reconnect");
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
      setStatus(state.connected ? "Connected; waiting for fresh LSL samples" : "Looking for saved sensor", state.connected ? "Connected" : "Reconnect");
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
      if (!state.mockMode && event.connected && event.deviceName && (state.selectedDeviceId || state.preferences.lastDevice?.id)) {
        const saved = { id: state.selectedDeviceId || state.preferences.lastDevice.id, name: event.deviceName };
        state.preferences.lastDevice = saved;
        state.preferences.recentDevices = [saved, ...(state.preferences.recentDevices || [])
          .filter((device) => device.id !== saved.id)].slice(0, 6);
        renderDevices();
      }
      renderConnection({
        deviceName: event.deviceName,
        lsl: state.lsl,
      });
      setStatus(event.connected ? "Connected; waiting for fresh LSL samples" : (event.message || "Disconnected"), event.connected ? "Connected" : "Idle");
    } else if (event.kind === "samples") {
      if (state.connected && isLslPublishing(event.lsl) && hasNewSamples(event, state.samples)) {
        state.lastSampleAt = performance.now();
      }
      state.samples = event;
      state.lsl = event.lsl || state.lsl;
      state.streaming = state.connected && isLslPublishing(state.lsl) && state.lastSampleAt > 0
        && performance.now() - state.lastSampleAt < 1200;
      renderConnection();
      if (state.streaming) {
        if (state.status !== "Live: sensor samples reaching LSL") setStatus("Live: sensor samples reaching LSL", "Live");
      } else if (state.connected && !isLslPublishing(state.lsl)) {
        setStatus(`Sensor connected; ${state.lsl || "LSL unavailable"}`, "LSL", true);
      }
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
      renderBootstrap({ productName: "Vernier Stream Mini", kind: "vernier", scanLabel: "Vernier Go Direct", preferences: state.preferences });
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
    elements["device-rescan"].addEventListener("click", scanDevices);
    elements["bluetooth-toggle"].addEventListener("change", changeRadio);
    elements["device-done"].addEventListener("click", () => elements["device-dialog"].close());
    elements["device-dialog-close"].addEventListener("click", () => elements["device-dialog"].close());
    elements["mock-button"].addEventListener("click", openMockNode);
    elements["connect-button"].addEventListener("click", connectSelected);
    elements["disconnect-button"].addEventListener("click", disconnect);
    elements["metrics-button"].addEventListener("click", () => {
      renderMetricDialog();
      elements["metrics-dialog"].showModal();
    });
    elements["apply-metrics"].addEventListener("click", (event) => {
      event.preventDefault();
      elements["metrics-dialog"].close();
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    bindElements();
    installHandlers();
    window.setInterval(() => {
      if (state.streaming && performance.now() - state.lastSampleAt >= 1200) {
        state.streaming = false;
        renderConnection();
        setStatus("No fresh samples; waiting for sensor", "Waiting", true);
      }
    }, 400);
    initNative().catch(reportError);
  });
})();
