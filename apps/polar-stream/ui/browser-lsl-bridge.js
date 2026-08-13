(() => {
  "use strict";

  const PROTOCOL_VERSION = 1;
  const MAX_QUEUED_EVENTS = 256;
  const MAX_BATCH_EVENTS = 24;
  const FLUSH_DELAY_MS = 20;

  function createClientId() {
    if (typeof window.crypto?.randomUUID === "function") {
      return window.crypto.randomUUID();
    }
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  }

  const state = {
    clientId: createClientId(),
    endpoint: null,
    token: null,
    connected: false,
    enabled: false,
    queue: [],
    sequence: 0,
    inFlight: false,
    flushTimer: 0,
    droppedEvents: 0,
    acceptedEvents: 0,
    acceptedSamples: 0,
    health: "Open this page from the installed Polar Stream app to pair native LSL.",
    lastError: null,
  };

  class BrowserLslBridgeError extends Error {
    constructor(code, message) {
      super(message);
      this.name = "BrowserLslBridgeError";
      this.code = code;
      this.retryable = true;
    }
  }

  function consumeFragmentCredentials() {
    const raw = window.location.hash.startsWith("#") ? window.location.hash.slice(1) : "";
    if (!raw) return;
    const parameters = new URLSearchParams(raw);
    const portText = parameters.get("bridgePort");
    const token = parameters.get("bridgeToken");
    parameters.delete("bridgePort");
    parameters.delete("bridgeToken");
    const remaining = parameters.toString();
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${window.location.search}${remaining ? `#${remaining}` : ""}`,
    );
    const port = Number(portText);
    if (!Number.isInteger(port) || port < 1 || port > 65535 || !/^[a-f0-9]{32}$/i.test(token || "")) {
      return;
    }
    state.endpoint = `http://127.0.0.1:${port}`;
    state.token = token;
    state.health = "Checking the authenticated desktop LSL bridge…";
  }

  function targetAddressSpaceOption() {
    if (!state.endpoint) return {};
    try {
      const request = new Request(`${state.endpoint}/v1/status`, { targetAddressSpace: "loopback" });
      return request.targetAddressSpace === "loopback" ? { targetAddressSpace: "loopback" } : {};
    } catch (_error) {
      return {};
    }
  }

  function snapshot() {
    return Object.freeze({
      paired: Boolean(state.endpoint && state.token),
      connected: state.connected,
      enabled: state.enabled,
      health: state.health,
      lastError: state.lastError,
      droppedEvents: state.droppedEvents,
      acceptedEvents: state.acceptedEvents,
      acceptedSamples: state.acceptedSamples,
    });
  }

  function announce() {
    window.dispatchEvent(new CustomEvent("polar-browser-lsl-status", { detail: snapshot() }));
  }

  async function request(path, { method = "GET", body, timeoutMs = 3000 } = {}) {
    if (!state.endpoint || !state.token) {
      throw new BrowserLslBridgeError(
        "BROWSER_LSL_BRIDGE_NOT_PAIRED",
        "Open the browser demo from the installed Polar Stream app to pair native LSL.",
      );
    }
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), timeoutMs);
    try {
      const response = await window.fetch(`${state.endpoint}${path}`, {
        method,
        mode: "cors",
        cache: "no-store",
        credentials: "omit",
        referrerPolicy: "no-referrer",
        headers: {
          Authorization: `Bearer ${state.token}`,
          "X-Polar-Bridge-Client": state.clientId,
          ...(body ? { "Content-Type": "application/json" } : {}),
        },
        body: body ? JSON.stringify(body) : undefined,
        signal: controller.signal,
        ...targetAddressSpaceOption(),
      });
      let payload = null;
      try {
        payload = await response.json();
      } catch (_error) {
        // A status code remains enough to produce a stable safe error below.
      }
      if (!response.ok) {
        throw new BrowserLslBridgeError(
          payload?.code || "BROWSER_LSL_BRIDGE_REQUEST_FAILED",
          payload?.message || `The desktop LSL bridge returned HTTP ${response.status}.`,
        );
      }
      return payload || {};
    } catch (error) {
      if (error instanceof BrowserLslBridgeError) throw error;
      if (error?.name === "AbortError") {
        throw new BrowserLslBridgeError(
          "BROWSER_LSL_BRIDGE_TIMEOUT",
          "The desktop LSL bridge did not respond in time.",
        );
      }
      throw new BrowserLslBridgeError(
        "BROWSER_LSL_BRIDGE_UNREACHABLE",
        "The paired desktop LSL bridge is not reachable. Keep Polar Stream open and allow Chromium local-network access.",
      );
    } finally {
      window.clearTimeout(timeout);
    }
  }

  async function initialize() {
    if (!state.endpoint || !state.token) {
      announce();
      return snapshot();
    }
    try {
      const status = await request("/v1/status");
      if (status.protocolVersion !== PROTOCOL_VERSION) {
        throw new BrowserLslBridgeError(
          "BROWSER_LSL_BRIDGE_VERSION_MISMATCH",
          "The installed app and browser demo use incompatible bridge versions.",
        );
      }
      state.connected = true;
      state.enabled = Boolean(status.lslEnabled);
      state.acceptedEvents = Number(status.acceptedEvents) || 0;
      state.acceptedSamples = Number(status.acceptedSamples) || 0;
      state.health = state.enabled
        ? "Paired desktop bridge · native LSL active"
        : "Paired desktop bridge ready · enable LSL when needed";
      state.lastError = null;
    } catch (error) {
      state.connected = false;
      state.enabled = false;
      state.health = error.message;
      state.lastError = error.message;
    }
    announce();
    return snapshot();
  }

  async function configure(config) {
    if (!config?.lslEnabled && !state.connected) {
      return {
        streamName: config?.streamName || "Polar-H10",
        lsl: state.health,
        osc: "Desktop app only · browser OSC is unavailable",
      };
    }
    if (!state.connected) await initialize();
    if (!state.connected) {
      throw new BrowserLslBridgeError(
        "BROWSER_LSL_BRIDGE_UNREACHABLE",
        state.health,
      );
    }
    const response = await request("/v1/config", {
      method: "POST",
      timeoutMs: 5000,
      body: {
        protocolVersion: PROTOCOL_VERSION,
        config: { ...config, oscEnabled: false },
      },
    });
    if (response.protocolVersion !== PROTOCOL_VERSION || !response.health) {
      throw new BrowserLslBridgeError(
        "BROWSER_LSL_BRIDGE_INVALID_RESPONSE",
        "The desktop LSL bridge returned an invalid configuration response.",
      );
    }
    state.enabled = Boolean(config.lslEnabled);
    state.health = state.enabled
      ? `Desktop bridge · ${response.health.lsl}`
      : "Paired desktop bridge ready · enable LSL when needed";
    state.lastError = null;
    if (!state.enabled) {
      state.queue.length = 0;
    }
    announce();
    return response.health;
  }

  function normalizeEvent(event) {
    if (!event || typeof event !== "object") return null;
    if (event.kind === "ecg" && Array.isArray(event.microvolts) && event.microvolts.length) {
      return {
        kind: "ecg",
        sensorTimestampNs: String(event.sensorTimestampNs ?? 0),
        microvolts: event.microvolts,
      };
    }
    if (event.kind === "accelerometer" && Array.isArray(event.samples) && event.samples.length) {
      return {
        kind: "accelerometer",
        sensorTimestampNs: String(event.sensorTimestampNs ?? 0),
        samples: event.samples.map((sample) => ({
          xMg: sample.xMg,
          yMg: sample.yMg,
          zMg: sample.zMg,
        })),
      };
    }
    if (event.kind === "metrics" && Array.isArray(event.values) && event.values.length) {
      return {
        kind: "metrics",
        values: event.values.map((metric) => ({ id: metric.id, value: metric.value })),
      };
    }
    return null;
  }

  function publish(event) {
    if (!state.enabled || !state.connected) return;
    const normalized = normalizeEvent(event);
    if (!normalized) return;
    if (state.queue.length >= MAX_QUEUED_EVENTS) {
      state.droppedEvents += 1;
      state.health = `Desktop bridge overloaded · ${state.droppedEvents} event(s) dropped`;
      state.lastError = state.health;
      announce();
      return;
    }
    state.queue.push(normalized);
    scheduleFlush();
  }

  function scheduleFlush() {
    if (state.inFlight || state.flushTimer || !state.queue.length) return;
    state.flushTimer = window.setTimeout(() => {
      state.flushTimer = 0;
      void flush();
    }, FLUSH_DELAY_MS);
  }

  async function flush() {
    if (state.inFlight || !state.enabled || !state.connected || !state.queue.length) return;
    const events = state.queue.splice(0, MAX_BATCH_EVENTS);
    state.inFlight = true;
    state.sequence += 1;
    try {
      const response = await request("/v1/events", {
        method: "POST",
        body: {
          protocolVersion: PROTOCOL_VERSION,
          sequence: state.sequence,
          events,
        },
      });
      state.acceptedEvents += Number(response.acceptedEvents) || 0;
      state.acceptedSamples += Number(response.acceptedSamples) || 0;
    } catch (error) {
      state.droppedEvents += events.length + state.queue.length;
      state.queue.length = 0;
      state.connected = false;
      state.enabled = false;
      state.health = error.message;
      state.lastError = error.message;
      announce();
    } finally {
      state.inFlight = false;
      scheduleFlush();
    }
  }

  consumeFragmentCredentials();
  window.PolarBrowserLslBridge = Object.freeze({
    initialize,
    configure,
    publish,
    status: snapshot,
  });
})();
