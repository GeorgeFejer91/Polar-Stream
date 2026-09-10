const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(path.join(__dirname, "..", "breathing-comparison.js"), "utf8");

function load() {
  const window = {};
  vm.runInNewContext(source, { window }, { filename: "breathing-comparison.js" });
  return window.PolarBreathingComparison;
}

function wavePoints(rateHz = 200, seconds = 30, invert = false) {
  return Array.from({ length: Math.floor(rateHz * seconds) + 1 }, (_, index) => {
    const timestamp = index / rateHz;
    const wave = Math.sin(timestamp * Math.PI * 2 / 4);
    return { timestamp, value: (invert ? -1 : 1) * wave, gapBefore: false };
  });
}

function packetized(points, sizes) {
  const packets = [];
  let offset = 0;
  let sizeIndex = 0;
  while (offset < points.length) {
    const size = Math.min(sizes[sizeIndex % sizes.length], points.length - offset);
    packets.push(points.slice(offset, offset + size).map((point) => ({ ...point })));
    offset += size;
    sizeIndex += 1;
  }
  return packets.flat();
}

test("resampled breathing is invariant to notification batch cadence", () => {
  const api = load();
  const physical = wavePoints(200, 8);
  const bounds = api.robustBounds(physical, 0.1);
  const one = api.resampleSeries(packetized(physical, [1]), {
    start: 2,
    end: 8,
    bounds,
    rateHz: 60,
    smoothingTauSeconds: 0.12,
    maxGapSeconds: 0.5,
  }).points;
  const batched = api.resampleSeries(packetized(physical, [36, 37, 35, 36]), {
    start: 2,
    end: 8,
    bounds,
    rateHz: 60,
    smoothingTauSeconds: 0.12,
    maxGapSeconds: 0.5,
  }).points;

  assert.equal(batched.length, one.length);
  assert.deepEqual(
    batched.map((point) => [point.timestamp, point.value]),
    one.map((point) => [point.timestamp, point.value]),
  );
});

test("sparse Vernier samples interpolate smoothly without forward extrapolation", () => {
  const api = load();
  const sparse = wavePoints(10, 5);
  const bounds = api.robustBounds(sparse, 0.1);
  const result = api.resampleSeries(sparse, {
    start: 0,
    end: 6,
    bounds,
    rateHz: 60,
    smoothingTauSeconds: 0.12,
    maxGapSeconds: 0.5,
  });

  assert.equal(result.end, 5);
  assert.ok(result.points.length > sparse.length * 4);
  assert.ok(result.points.at(-1).timestamp <= sparse.at(-1).timestamp);
  assert.ok(result.points.every((point) => point.value >= 0 && point.value <= 1));
  const maximumStep = Math.max(...result.points.slice(1).map((point, index) => (
    Math.abs(point.value - result.points[index].value)
  )));
  assert.ok(maximumStep < 0.08, `renderer smoothing stepped by ${maximumStep}`);
});

test("resampling breaks the path across a source gap", () => {
  const api = load();
  const points = [
    { timestamp: 0, value: 0 },
    { timestamp: 0.1, value: 0.2 },
    { timestamp: 1, value: 0.8, gapBefore: true },
    { timestamp: 1.1, value: 1 },
  ];
  const result = api.resampleSeries(points, {
    start: 0,
    end: 1.1,
    bounds: { ready: true, lower: 0, upper: 1, span: 1 },
    rateHz: 20,
    maxGapSeconds: 0.5,
  }).points;

  assert.ok(result.some((point) => point.gapBefore));
  assert.equal(result.some((point) => point.timestamp > 0.1 && point.timestamp < 1), false);
});

test("zero-lag Vernier evidence identifies a stable inverted H10 mounting", () => {
  const api = load();
  const reference = wavePoints(10, 30, false);
  const polar = wavePoints(10, 30, true).map((point) => ({ ...point, value: point.value * 0.04 + 0.7 }));
  const evidence = api.alignmentEvidence(reference, polar, {
    minOverlapSeconds: 15,
    minimumPairs: 100,
    minimumReversals: 4,
    referenceMinimumSpan: 0.1,
    candidateMinimumSpan: 0.0025,
  });

  assert.equal(evidence.ready, true);
  assert.equal(evidence.status, "candidate");
  assert.equal(evidence.sign, -1);
  assert.ok(evidence.correlation < -0.99);
});

test("flat and short traces cannot acquire an automatic direction", () => {
  const api = load();
  const reference = wavePoints(10, 30);
  const flat = reference.map((point) => ({ ...point, value: 1 }));
  const short = wavePoints(10, 4, true);

  assert.equal(api.alignmentEvidence(reference, flat, {
    referenceMinimumSpan: 0.1,
    candidateMinimumSpan: 0.08,
  }).status, "flat");
  assert.equal(api.alignmentEvidence(reference, short, {
    referenceMinimumSpan: 0.1,
    candidateMinimumSpan: 0.1,
  }).status, "learning");
});
