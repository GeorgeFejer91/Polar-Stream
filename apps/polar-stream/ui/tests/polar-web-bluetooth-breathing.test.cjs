const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(path.join(__dirname, "..", "polar-web-bluetooth.js"), "utf8");
function load() {
  const window = { setTimeout, clearTimeout };
  const context = {
    window,
    navigator: { userAgent: "Node", platform: "Node" },
    document: { addEventListener() {}, removeEventListener() {}, visibilityState: "visible" },
    performance: { now: () => 0 },
    BigInt,
  };
  vm.runInNewContext(source, context, { filename: "polar-web-bluetooth.js" });
  return window.PolarWebBluetooth;
}

function sampleAt(index) {
  const t = index / 200;
  const breath = 0.16 * Math.sin(2 * Math.PI * t / 4);
  return { xMg: Math.round(1000 * breath), yMg: 0, zMg: Math.round(1000 * breath * 0.7) };
}

function replay(chunkSizes, settings = {}) {
  const api = load();
  const processor = api.createTimedBreathingProcessor({
    calibrationWindowSeconds: 2,
    staleTimeoutSeconds: 0.5,
    adaptiveBounds: false,
    ...settings,
  });
  let index = 0;
  let snapshot;
  let chunkIndex = 0;
  while (index < 4000) {
    const size = chunkSizes[chunkIndex % chunkSizes.length];
    const samples = Array.from({ length: Math.min(size, 4000 - index) }, (_, offset) => sampleAt(index + offset));
    const newestNs = BigInt(Math.round((index + samples.length - 1) * 5_000_000));
    snapshot = processor.pushTimed(samples, newestNs);
    index += samples.length;
    chunkIndex += 1;
  }
  return { processor, snapshot };
}

test("timed PCA output is invariant across BLE notification chunking", () => {
  const one = replay([1]).snapshot;
  for (const chunks of [[10], [37], [3, 17, 41, 9]]) {
    const result = replay(chunks).snapshot;
    assert.ok(Math.abs(result.volume01 - one.volume01) < 0.02);
    assert.ok(Math.abs(result.magnitudeG - one.magnitudeG) < 0.02);
  }
});

test("backward source timestamps are late drops, while a real gap becomes Lost", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 1 });
  const batch = Array.from({ length: 37 }, (_, i) => sampleAt(i));
  const first = processor.pushTimed(batch, 180_000_000n);
  const backward = processor.pushTimed(batch, 100_000_000n);
  assert.equal(backward.lost, false);
  assert.ok(backward.diagnostics.lateDropped > 0);
  const gap = processor.pushTimed(batch, 2_000_000_000n);
  assert.equal(gap.lost, true);
  assert.ok(gap.diagnostics.forwardGaps > 0);
  assert.ok(first);
});

test("source clock regression resets timing state and accepts the new epoch", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 1 });
  const batch = Array.from({ length: 37 }, (_, i) => sampleAt(i));
  processor.pushTimed(batch, 1_000_000_000n);
  const snapshot = processor.pushTimed(batch, 100_000_000n);
  assert.equal(snapshot.lost, false);
  assert.equal(snapshot.diagnostics.resets, 1);
  assert.ok(snapshot.diagnostics.accepted > 0);
});

test("hysteresis confirmation and dwell suppress a short reversal", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 1, phaseConfirmationSeconds: 0.4, phaseMinimumDwellSeconds: 0.4 });
  const batch = Array.from({ length: 37 }, (_, i) => sampleAt(i));
  for (let i = 0; i < 100; i += 1) processor.pushTimed(batch, BigInt((i + 1) * 185_000_000));
  const before = processor.phase;
  for (let i = 0; i < 2; i += 1) processor.pushTimed(batch, BigInt((i + 102) * 185_000_000));
  assert.ok(processor.phase === before || processor.phase === 0);
});

test("adaptive bounds follow source-time quantiles without changing phase state", () => {
  const api = load();
  const processor = replay([37], { adaptiveBounds: true, calibrationWindowSeconds: 4 }).processor;
  const fixed = replay([37], { adaptiveBounds: false, calibrationWindowSeconds: 4 }).processor;
  const start = 4000;
  for (let index = start; index < start + 5000; index += 37) {
    const samples = Array.from({ length: Math.min(37, start + 5000 - index) }, (_, offset) => {
      const sample = sampleAt(index + offset);
      return { ...sample, xMg: sample.xMg + 30, zMg: sample.zMg + 21 };
    });
    const newest = BigInt(Math.round((index + samples.length - 1) * 5_000_000));
    const adaptiveSnapshot = processor.pushTimed(samples, newest);
    const fixedSnapshot = fixed.pushTimed(samples, newest);
    assert.equal(adaptiveSnapshot.phase, fixedSnapshot.phase);
  }
  assert.notEqual(processor.boundMin, fixed.boundMin);
  assert.equal(processor.calibrated, true);
});

test("legacy-v0 configuration selects the compatibility processor", () => {
  const api = load();
  const processor = api.createConfiguredBreathingProcessor({ volumeMode: "legacy-v0", stateMode: "legacy-v0" });
  assert.equal(typeof processor.pushTimed, "undefined");
  assert.equal(typeof processor.push, "function");
});

test("timed PCA seeds the dominant covariance axis for a Y/Z-only signal", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 1, axes: [false, true, true], adaptiveBounds: false });
  const samples = Array.from({ length: 300 }, (_, i) => {
    const wave = Math.round(Math.sin(i / 18) * 120);
    return { xMg: 0, yMg: wave, zMg: Math.round(wave * 0.4) };
  });
  processor.pushTimed(samples, 1_495_000_000n);
  assert.equal(processor.calibrated, true);
  assert.ok(Math.abs(processor.axis[1]) > Math.abs(processor.axis[2]));
  assert.ok(processor.axis[1] > 0, "largest component sign is deterministic before inversion");
});

test("timed readiness and confidence expose motion and PCA quality", () => {
  const result = replay([37]).snapshot;
  assert.equal(typeof result.diagnostics.motionScore, "number");
  assert.equal(typeof result.diagnostics.pcaDominance01, "number");
  assert.ok(result.diagnostics.motionScore >= 0 && result.diagnostics.motionScore <= 1);
  assert.ok(result.diagnostics.pcaDominance01 >= 0 && result.diagnostics.pcaDominance01 <= 1);
  assert.ok(result.values.find((value) => value.id === "breathing_signal_confidence").value <= 1);
});

test("confirmation and active dwell are independent gates", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ phaseConfirmationSeconds: 0.1, phaseMinimumDwellSeconds: 0.5 });
  processor.phase = 1;
  processor.activeSinceNs = 0n;
  processor.classifyDerivative(-1, 200_000_000n);
  processor.classifyDerivative(-1, 300_000_000n);
  assert.equal(processor.phase, 1, "confirmation alone cannot bypass active dwell");
  processor.classifyDerivative(-1, 600_000_000n);
  assert.equal(processor.phase, -1);
});

test("36-sample physical cadence interpolates between newest PMD anchors", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 1, adaptiveBounds: false });
  const batch = Array.from({ length: 36 }, (_, i) => sampleAt(i));
  for (let index = 0; index < 8; index += 1) {
    processor.pushTimed(batch, BigInt(1_000_000_000 + index * 177_800_000));
  }
  const second = processor.pushTimed(batch, 2_422_400_000n);
  assert.equal(second.sensorTimestampNs, "2422400000");
  assert.equal(second.presentationPoints.at(-1).sourceTimestampNs, "2422400000");
  const prior = second.presentationPoints.at(-36);
  assert.equal(prior.sourceTimestampNs, "2249538888");
  assert.ok(Number(second.presentationPoints.at(-1).sourceTimestampNs) - Number(prior.sourceTimestampNs) > 170_000_000);
});

test("gap detection uses the batch boundary, not the full notification duration", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 0.5, staleTimeoutSeconds: 0.01, adaptiveBounds: false });
  const batch = Array.from({ length: 36 }, (_, i) => sampleAt(i));
  processor.pushTimed(batch, 1_000_000_000n);
  // The 177.8ms anchor interval exceeds staleTimeout, but its nominal first
  // sample is only 5ms after the preceding newest sample.
  const times = processor.sourceTimes(batch, 1_177_800_000n, true);
  assert.equal(times.at(-1), 1_177_800_000n);
  assert.equal(times.at(-1) - times.at(-2), 4_938_889n);
  const result = processor.pushTimed(batch, 1_177_800_000n);
  assert.equal(result.lost, false);
  assert.equal(result.diagnostics.forwardGaps, 0);
});

test("genuine boundary gaps retain nominal reconstruction and report Lost", () => {
  const api = load();
  const processor = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 0.5, staleTimeoutSeconds: 0.5, adaptiveBounds: false });
  const batch = Array.from({ length: 36 }, (_, i) => sampleAt(i));
  processor.pushTimed(batch, 1_000_000_000n);
  const result = processor.pushTimed(batch, 2_000_000_000n);
  assert.equal(result.lost, true);
  const nominal = processor.sourceTimes(batch, 2_000_000_000n, false);
  assert.equal(nominal.at(-1) - nominal.at(-2), 5_000_000n);
});

test("decoded PMD timestamp is preserved exactly and timed output exposes source points", () => {
  const api = load();
  const bytes = new Uint8Array(16);
  bytes[0] = 2;
  let timestamp = 123456789n;
  for (let i = 0; i < 8; i += 1) { bytes[1 + i] = Number(timestamp & 255n); timestamp >>= 8n; }
  bytes[9] = 1;
  new DataView(bytes.buffer).setInt16(10, 100, true);
  new DataView(bytes.buffer).setInt16(12, 0, true);
  new DataView(bytes.buffer).setInt16(14, 50, true);
  const frame = api.decodePmd(bytes);
  assert.equal(frame.sensorTimestampNs, "123456789");
  const snapshot = api.createTimedBreathingProcessor({ calibrationWindowSeconds: 1 }).pushTimed(frame.samples, frame.sensorTimestampNs);
  assert.equal(snapshot.sensorTimestampNs, "123456789");
  assert.ok(Array.isArray(snapshot.presentationPoints));
});

test("presenters are bounded and do not alter canonical events", () => {
  const api = load();
  const fresh = api.createBreathingPresentation("fresh+smoothing");
  const delayed = api.createBreathingPresentation("timestamp-faithful", { delaySeconds: 0.18 });
  const point = (time, volume) => ({ sensorTimestampNs: String(Math.round(time * 1e9)), volume01: volume });
  const first = fresh.push(point(0, 0));
  for (let i = 1; i <= 100; i += 1) { fresh.push(point(i / 200, 1)); delayed.push(point(i / 200, 1)); }
  assert.equal(first, 0);
  assert.ok(fresh.value >= 0 && fresh.value <= 1);
  assert.ok(delayed.value >= 0 && delayed.value <= 1);
  assert.ok(delayed.points.length <= 512);
});
