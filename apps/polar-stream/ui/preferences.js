(() => {
  "use strict";

  const STORAGE_KEY = "polar-stream.preferences.v1";
  const defaults = Object.freeze({
    streamName: null,
    lastDevice: null,
    outputConfig: null,
    keepVernierAwake: false,
  });

  function load() {
    try {
      const stored = JSON.parse(window.localStorage.getItem(STORAGE_KEY) || "null");
      const streamName = typeof stored?.streamName === "string" && stored.streamName
        ? stored.streamName
        : null;
      const lastDevice = typeof stored?.lastDevice?.id === "string"
        && typeof stored?.lastDevice?.name === "string"
        ? { id: stored.lastDevice.id, name: stored.lastDevice.name }
        : null;
      const outputConfig = stored?.outputConfig && typeof stored.outputConfig === "object"
        && Array.isArray(stored.outputConfig.outputs)
        ? structuredClone(stored.outputConfig)
        : null;
      const keepVernierAwake = stored?.keepVernierAwake === true;
      return { streamName, lastDevice, outputConfig, keepVernierAwake };
    } catch (_error) {
      return { ...defaults };
    }
  }

  function write(preferences) {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
    } catch (_error) {
      // The app remains fully usable if WebView storage is disabled.
    }
    return preferences;
  }

  function saveLastDevice(device) {
    return write({ ...load(), lastDevice: { id: device.id, name: device.name } });
  }

  function saveOutputConfig(outputConfig) {
    return write({ ...load(), streamName: outputConfig.streamName, outputConfig: structuredClone(outputConfig) });
  }

  function saveKeepVernierAwake(keepVernierAwake) {
    return write({ ...load(), keepVernierAwake: keepVernierAwake === true });
  }

  window.PolarPreferences = Object.freeze({
    STORAGE_KEY,
    load,
    saveLastDevice,
    saveOutputConfig,
    saveKeepVernierAwake,
  });
})();
