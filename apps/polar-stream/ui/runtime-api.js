(() => {
  "use strict";

  const core = window.__TAURI__?.core;
  const isNative = Boolean(core?.invoke && core?.Channel);

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

  const api = {
    isNative,
    formatError(error) {
      const normalized = normalizeError(error);
      return normalized.code === "NATIVE_OPERATION_FAILED"
        ? normalized.message
        : `${normalized.message} (${normalized.code})`;
    },
    async getBootstrap(fallback) {
      return isNative ? invoke("get_bootstrap") : fallback;
    },
    async migrateLegacyPreferences(legacy) {
      return isNative ? invoke("migrate_legacy_preferences", { legacy }) : legacy;
    },
    async scanDevices() {
      if (isNative) return invoke("scan_devices");
      return previewDelay([
        { id: "preview-h10-a", name: "Polar H10 8F3A2C1B", rssi: -48 },
        { id: "preview-h10-b", name: "Polar H10 4D9E7A20", rssi: -63 },
      ], 850);
    },
    async connectDevice(deviceId, onEvent) {
      if (!isNative) return previewDelay(undefined, 450);
      const events = new core.Channel();
      events.onmessage = onEvent;
      return invoke("connect_device", { deviceId, events });
    },
    async disconnectDevice() {
      return isNative ? invoke("disconnect_device") : undefined;
    },
    async updateOutputConfig(config) {
      if (isNative) return invoke("update_output_config", { config });
      return {
        streamName: config.streamName,
        lsl: config.lslEnabled ? "Preview · liblsl is checked in the native app" : "Off",
        osc: config.oscEnabled ? "Preview · UDP localhost:9000" : "Off",
      };
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
