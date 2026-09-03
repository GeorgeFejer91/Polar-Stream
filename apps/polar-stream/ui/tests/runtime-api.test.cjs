const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const runtimeSource = readFileSync(join(__dirname, "..", "runtime-api.js"), "utf8");

function loadNativeRuntime(invoke) {
  class Channel {
    onmessage = null;
  }
  const window = {
    location: { search: "" },
    __TAURI__: { core: { invoke, Channel } },
    PolarSourcePalettes: [],
    setTimeout,
  };
  const context = vm.createContext({
    window,
    URL,
    URLSearchParams,
    Error,
    Promise,
    Set,
    Map,
    Object,
    Boolean,
    String,
    structuredClone,
  });
  vm.runInContext(runtimeSource, context, { filename: "runtime-api.js" });
  return window.PolarRuntimeApi;
}

function config(streamName) {
  return {
    streamName,
    lslEnabled: false,
    oscEnabled: false,
    csvEnabled: false,
    audioEnabled: false,
    outputs: ["raw_ecg", "raw_acc"],
    metricOptions: {},
    customFormulas: [],
  };
}

test("a rejected native output update does not poison the configuration used by a later source", async () => {
  const updates = [];
  const invoke = async (command, payload = {}) => {
    if (command === "update_output_config") {
      updates.push(structuredClone(payload.config));
      if (payload.config.streamName === "Rejected") {
        throw { code: "OUTPUT_RECONFIGURE_FAILED", message: "injected native rejection" };
      }
      return {
        streamName: payload.config.streamName,
        lsl: "Off",
        osc: "Off",
        csv: "Off",
        audio: "Off",
      };
    }
    if (command === "connect_device") return { id: "native-source-1" };
    throw new Error(`Unexpected command: ${command}`);
  };
  const runtime = loadNativeRuntime(invoke);

  await runtime.updateOutputConfig(config("Accepted"));
  await assert.rejects(
    runtime.updateOutputConfig(config("Rejected")),
    (error) => error.code === "OUTPUT_RECONFIGURE_FAILED",
  );
  await runtime.connectDevice("native-device-1", () => {});

  assert.deepEqual(
    updates.map((update) => update.streamName),
    ["Accepted", "Rejected", "Accepted"],
  );
});
