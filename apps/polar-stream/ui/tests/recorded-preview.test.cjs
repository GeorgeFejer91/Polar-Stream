const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const ui = path.resolve(__dirname, "..");
function browserAsset(filename, globalName) {
  const context = { window: {} };
  vm.runInNewContext(fs.readFileSync(path.join(ui, filename), "utf8"), context, { filename });
  return context.window[globalName] || context[globalName];
}

const fixtureApi = browserAsset("preview-fixture.js", "PolarPreviewFixture");
const formulaPreview = browserAsset("formula-preview.js", "PolarFormulaPreview");
const fixture = fixtureApi.validateFixture(JSON.parse(fs.readFileSync(path.join(ui, "data/preview-recording.json"), "utf8")));
const previews = browserAsset("metric-previews.js", "PolarMetricPreviews");
const catalog = browserAsset("metric-catalog.js", "PolarMetricCatalog");

test("canonical preview fixture is a complete anonymized real H10 recording", () => {
  assert.equal(fixture.source, "real-polar-h10-recording");
  assert.equal(fixture.durationMs, 60_000);
  assert.equal(fixture.ecg.microvolts.length, 7_800);
  assert.equal(fixture.accelerometer.samples.length, 12_000);
  assert.equal(fixture.metricEvents.length, 60);
  for (const forbidden of ["address", "serial", "username", "userName", "owner"]) {
    assert.equal(Object.hasOwn(fixture, forbidden), false);
  }
});

test("recorded playback crosses its circular seam without a value or time break", () => {
  let now = 0;
  let scheduled = null;
  const emitted = [];
  const player = new fixtureApi.LoopPlayer(fixture, (event) => emitted.push(event), {
    now: () => now,
    schedule: (callback) => { scheduled = callback; return 1; },
    cancel: () => {},
  });
  player.start();
  now = fixture.durationMs - 1;
  scheduled();
  now = fixture.durationMs + 8;
  scheduled();
  player.stop();

  const ecg = emitted.filter((event) => event.kind === "ecg");
  const lastEcg = ecg.filter((event) => event.recordedPreviewLoop === 0).at(-1);
  const firstRepeatedEcg = ecg.find((event) => event.recordedPreviewLoop === 1);
  assert.equal(lastEcg.microvolts.at(-1), firstRepeatedEcg.microvolts[0]);
  assert.equal(firstRepeatedEcg.sensorTimestampNs - lastEcg.sensorTimestampNs, Math.round(1_000_000_000 / fixture.ecg.sampleRateHz));
  assert.equal(firstRepeatedEcg.seamlessPreview, true);

  const acc = emitted.filter((event) => event.kind === "accelerometer");
  const lastAcc = acc.filter((event) => event.recordedPreviewLoop === 0).at(-1).samples.at(-1);
  const firstRepeatedAcc = acc.find((event) => event.recordedPreviewLoop === 1).samples[0];
  assert.deepEqual(lastAcc, firstRepeatedAcc);
  assert.notEqual(fixture.ecg.microvolts.at(-1), fixture.ecg.microvolts[0], "the canonical recording was unexpectedly modified");
});

test("every catalog metric has a recorded numeric preview and mathematical definition", () => {
  assert.equal(previews.source.model, fixture.source);
  assert.equal(JSON.stringify(Object.keys(previews.metrics)), JSON.stringify(Array.from(catalog, (metric) => metric.id)));
  for (const metric of catalog) {
    assert.ok(metric.formula && metric.formula.length > 8, `${metric.id} lacks a mathematical definition`);
    assert.ok(metric.explainer && metric.explainer.length > 20, `${metric.id} lacks scientific context`);
    assert.match(metric.citationUrl, /^https:\/\//, `${metric.id} lacks an HTTPS citation`);
    const preview = previews.metrics[metric.id];
    assert.ok(preview.channels.length >= 1, `${metric.id} lacks preview channels`);
    for (const channel of preview.channels) {
      assert.ok(channel.values.length >= 20, `${metric.id} preview is too short`);
      assert.ok(channel.values.every(Number.isFinite), `${metric.id} preview is not finite`);
    }
  }
});

test("formula templates execute against the same recorded fixture", () => {
  for (const metric of catalog.filter((entry) => entry.formulaTemplate)) {
    const result = formulaPreview.preview(fixture, {
      id: `preview-${metric.id}`,
      name: `${metric.streamSuffix}_custom`,
      source: metric.formulaSource,
      expression: metric.formulaTemplate,
      unit: metric.unit,
      enabled: true,
    });
    assert.ok(result.output.length > 0, `${metric.id} template produced no output`);
    assert.ok(Number.isFinite(result.current), `${metric.id} template produced no current value`);
    assert.equal(result.output[0].value, result.output.at(-1).value, `${metric.id} formula preview is not circular`);
  }
});

test("categorical formula previews keep stepped integer classes at the loop seam", () => {
  const result = formulaPreview.preview(fixture, {
    id: "preview-phase",
    name: "phase",
    source: "accelerometer",
    expression: "breathing_phase(x, y, z, true, false, true, 0.75, 0.6, false)",
    unit: "class",
    enabled: true,
  });
  assert.equal(result.outputStep, true);
  assert.ok(result.output.every((sample) => Number.isInteger(sample.value)));
  assert.equal(result.output[0].value, result.output.at(-1).value);
});

test("window and normalization settings visibly change recorded formula output", () => {
  const base = { id: "preview-window", name: "Window", source: "ecg", unit: "µV", enabled: true };
  const short = formulaPreview.preview(fixture, { ...base, expression: "moving_mean(ecg, 0.1)" });
  const long = formulaPreview.preview(fixture, { ...base, expression: "moving_mean(ecg, 2)" });
  assert.notEqual(short.current, long.current);
  const normalized = formulaPreview.preview(fixture, { ...base, expression: "moving_mean(ecg, 0.1)" }, {
    normalization: "slidingWindow",
    windowSeconds: 5,
  });
  assert.ok(normalized.output.every((sample) => sample.value >= 0 && sample.value <= 1));
  assert.notEqual(normalized.current, short.current);
});
