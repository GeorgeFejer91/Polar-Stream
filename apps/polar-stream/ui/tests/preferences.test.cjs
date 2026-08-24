const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(path.join(__dirname, "..", "preferences.js"), "utf8");

function loadPreferences(initial = null) {
  const values = new Map();
  if (initial !== null) values.set("polar-stream.preferences.v1", JSON.stringify(initial));
  const window = {
    localStorage: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    },
  };
  vm.runInNewContext(source, { structuredClone, window });
  return window.PolarPreferences;
}

test("Vernier keep-connected policy is opt-in", () => {
  const preferences = loadPreferences();
  assert.equal(preferences.load().keepVernierAwake, false);
});

test("saving Vernier keep-connected policy preserves other preferences", () => {
  const preferences = loadPreferences({
    streamName: "participant_07",
    lastDevice: { id: "vernier:opaque-device", name: "GDX-RB" },
    outputConfig: { outputs: ["raw_force"] },
  });

  preferences.saveKeepVernierAwake(true);
  assert.deepEqual(JSON.parse(JSON.stringify(preferences.load())), {
    streamName: "participant_07",
    lastDevice: { id: "vernier:opaque-device", name: "GDX-RB" },
    outputConfig: { outputs: ["raw_force"] },
    keepVernierAwake: true,
  });
});
