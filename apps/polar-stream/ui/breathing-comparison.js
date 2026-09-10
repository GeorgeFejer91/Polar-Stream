(() => {
  "use strict";

  const DEFAULT_MAX_GAP_SECONDS = 0.5;
  const DEFAULT_RESAMPLE_RATE_HZ = 60;

  function clamp01(value) {
    return Math.max(0, Math.min(1, value));
  }

  function quantile(sorted, fraction) {
    if (!sorted.length) return Number.NaN;
    const position = (sorted.length - 1) * fraction;
    const low = Math.floor(position);
    const high = Math.ceil(position);
    return sorted[low] + (sorted[high] - sorted[low]) * (position - low);
  }

  function finiteOrderedPoints(points) {
    const clean = [];
    for (const point of points || []) {
      const timestamp = Number(point?.timestamp);
      const value = Number(point?.value);
      if (!Number.isFinite(timestamp) || !Number.isFinite(value)) continue;
      if (clean.length && timestamp <= clean.at(-1).timestamp) continue;
      clean.push({ timestamp, value, gapBefore: Boolean(point?.gapBefore) });
    }
    return clean;
  }

  function robustBounds(points, minimumSpan = 0, lowerQuantile = 0.05, upperQuantile = 0.95) {
    const values = (points || [])
      .map((point) => Number(point?.value))
      .filter(Number.isFinite)
      .sort((left, right) => left - right);
    if (values.length < 2) {
      return { ready: false, lower: Number.NaN, upper: Number.NaN, span: 0 };
    }
    const lower = quantile(values, lowerQuantile);
    const upper = quantile(values, upperQuantile);
    const span = upper - lower;
    return {
      ready: Number.isFinite(span)
        && span >= Math.max(Number.EPSILON, Number(minimumSpan) || 0),
      lower,
      upper,
      span: Number.isFinite(span) ? span : 0,
    };
  }

  function normalizedValue(value, bounds, invert = false) {
    const span = bounds.upper - bounds.lower;
    const normalized = span > Number.EPSILON ? clamp01((value - bounds.lower) / span) : 0.5;
    return invert ? 1 - normalized : normalized;
  }

  function interpolatedSample(points, timestamp, cursor, maxGapSeconds) {
    let rightIndex = cursor;
    while (rightIndex < points.length && points[rightIndex].timestamp < timestamp) rightIndex += 1;
    if (rightIndex >= points.length) return { sample: null, cursor: rightIndex };
    const right = points[rightIndex];
    if (Math.abs(right.timestamp - timestamp) <= 1e-9) {
      return {
        sample: { value: right.value, gapBefore: right.gapBefore },
        cursor: rightIndex,
      };
    }
    if (rightIndex === 0) return { sample: null, cursor: rightIndex };
    const left = points[rightIndex - 1];
    const interval = right.timestamp - left.timestamp;
    if (right.gapBefore || interval <= 0 || interval > maxGapSeconds) {
      return { sample: null, cursor: rightIndex };
    }
    const ratio = clamp01((timestamp - left.timestamp) / interval);
    return {
      sample: {
        value: left.value + (right.value - left.value) * ratio,
        gapBefore: false,
      },
      cursor: rightIndex,
    };
  }

  function resampleSeries(points, options = {}) {
    const source = finiteOrderedPoints(points);
    if (source.length < 2) return { points: [], bounds: robustBounds(source), end: null };
    const requestedStart = Number(options.start);
    const requestedEnd = Number(options.end);
    const start = Math.max(source[0].timestamp, Number.isFinite(requestedStart) ? requestedStart : source[0].timestamp);
    // Deliberately stop at the newest observed sample. Holding the trace here is
    // honest; projecting it into the future would invent breathing data.
    const end = Math.min(source.at(-1).timestamp, Number.isFinite(requestedEnd) ? requestedEnd : source.at(-1).timestamp);
    if (!(end >= start)) return { points: [], bounds: robustBounds(source), end: null };

    const boundsPoints = source.filter((point) => point.timestamp >= start && point.timestamp <= end);
    const bounds = options.bounds || robustBounds(
      boundsPoints,
      Number(options.minimumSpan) || 0,
      Number(options.lowerQuantile) || 0.05,
      Number(options.upperQuantile) || 0.95,
    );
    if (!bounds.ready) return { points: [], bounds, end };

    const rateHz = Math.max(1, Number(options.rateHz) || DEFAULT_RESAMPLE_RATE_HZ);
    const step = 1 / rateHz;
    const maxGapSeconds = Math.max(step, Number(options.maxGapSeconds) || DEFAULT_MAX_GAP_SECONDS);
    const tau = Math.max(0, Number(options.smoothingTauSeconds) || 0);
    const invert = Boolean(options.invert);
    const sampleTimes = [];
    let timestamp = Math.ceil((start - 1e-9) * rateHz) / rateHz;
    for (; timestamp <= end + 1e-9; timestamp += step) sampleTimes.push(Math.min(timestamp, end));
    if (!sampleTimes.length || end - sampleTimes.at(-1) > 1e-6) sampleTimes.push(end);

    const output = [];
    let cursor = 0;
    let smoothed = null;
    let lastTimestamp = null;
    let pendingGap = false;
    for (const sampleTime of sampleTimes) {
      const result = interpolatedSample(source, sampleTime, cursor, maxGapSeconds);
      cursor = result.cursor;
      if (!result.sample) {
        pendingGap = output.length > 0;
        smoothed = null;
        lastTimestamp = null;
        continue;
      }
      const target = normalizedValue(result.sample.value, bounds, invert);
      if (smoothed == null || result.sample.gapBefore || pendingGap || tau === 0) {
        smoothed = target;
      } else {
        const dt = Math.max(0, sampleTime - lastTimestamp);
        const alpha = dt <= 0 ? 1 : dt / (tau + dt);
        smoothed += alpha * (target - smoothed);
      }
      output.push({
        timestamp: sampleTime,
        value: clamp01(smoothed),
        gapBefore: Boolean(result.sample.gapBefore || pendingGap),
      });
      pendingGap = false;
      lastTimestamp = sampleTime;
    }
    return { points: output, bounds, end };
  }

  function pairedValues(reference, candidate, options) {
    const start = Math.max(reference[0].timestamp, candidate[0].timestamp, options.start);
    const end = Math.min(reference.at(-1).timestamp, candidate.at(-1).timestamp, options.end);
    if (!(end > start)) return { reference: [], candidate: [], start, end };
    const shared = {
      start,
      end,
      rateHz: options.rateHz,
      maxGapSeconds: options.maxGapSeconds,
      smoothingTauSeconds: 0,
    };
    const left = resampleSeries(reference, { ...shared, bounds: options.referenceBounds }).points;
    const right = resampleSeries(candidate, { ...shared, bounds: options.candidateBounds }).points;
    const referenceValues = [];
    const candidateValues = [];
    let leftIndex = 0;
    let rightIndex = 0;
    const tolerance = 0.25 / options.rateHz;
    while (leftIndex < left.length && rightIndex < right.length) {
      const delta = left[leftIndex].timestamp - right[rightIndex].timestamp;
      if (Math.abs(delta) <= tolerance) {
        if (!left[leftIndex].gapBefore && !right[rightIndex].gapBefore) {
          referenceValues.push(left[leftIndex].value);
          candidateValues.push(right[rightIndex].value);
        }
        leftIndex += 1;
        rightIndex += 1;
      } else if (delta < 0) leftIndex += 1;
      else rightIndex += 1;
    }
    return { reference: referenceValues, candidate: candidateValues, start, end };
  }

  function pearson(left, right) {
    const count = Math.min(left.length, right.length);
    if (count < 3) return Number.NaN;
    let leftMean = 0;
    let rightMean = 0;
    for (let index = 0; index < count; index += 1) {
      leftMean += left[index];
      rightMean += right[index];
    }
    leftMean /= count;
    rightMean /= count;
    let covariance = 0;
    let leftEnergy = 0;
    let rightEnergy = 0;
    for (let index = 0; index < count; index += 1) {
      const leftDelta = left[index] - leftMean;
      const rightDelta = right[index] - rightMean;
      covariance += leftDelta * rightDelta;
      leftEnergy += leftDelta * leftDelta;
      rightEnergy += rightDelta * rightDelta;
    }
    const denominator = Math.sqrt(leftEnergy * rightEnergy);
    return denominator > 1e-12 ? covariance / denominator : Number.NaN;
  }

  function slopeReversals(values, threshold = 0.005) {
    let previousDirection = 0;
    let reversals = 0;
    for (let index = 1; index < values.length; index += 1) {
      const delta = values[index] - values[index - 1];
      const direction = delta > threshold ? 1 : delta < -threshold ? -1 : 0;
      if (!direction) continue;
      if (previousDirection && direction !== previousDirection) reversals += 1;
      previousDirection = direction;
    }
    return reversals;
  }

  function alignmentEvidence(referencePoints, candidatePoints, options = {}) {
    const reference = finiteOrderedPoints(referencePoints);
    const candidate = finiteOrderedPoints(candidatePoints);
    const minOverlapSeconds = Math.max(1, Number(options.minOverlapSeconds) || 15);
    const rateHz = Math.max(1, Number(options.rateHz) || 10);
    const minimumPairs = Math.max(20, Number(options.minimumPairs) || 100);
    const referenceBounds = robustBounds(reference, Number(options.referenceMinimumSpan) || 0);
    const candidateBounds = robustBounds(candidate, Number(options.candidateMinimumSpan) || 0);
    if (!referenceBounds.ready || !candidateBounds.ready) {
      return { ready: false, status: "flat", reason: "flat or saturated signal", referenceBounds, candidateBounds };
    }
    if (reference.length < 2 || candidate.length < 2) {
      return { ready: false, status: "learning", reason: "waiting for overlapping samples", referenceBounds, candidateBounds };
    }
    const overlapStart = Math.max(reference[0].timestamp, candidate[0].timestamp);
    const overlapEnd = Math.min(reference.at(-1).timestamp, candidate.at(-1).timestamp);
    const overlapSeconds = Math.max(0, overlapEnd - overlapStart);
    if (overlapSeconds < minOverlapSeconds) {
      return { ready: false, status: "learning", reason: "collecting overlap", overlapSeconds, referenceBounds, candidateBounds };
    }
    const paired = pairedValues(reference, candidate, {
      start: overlapEnd - minOverlapSeconds,
      end: overlapEnd,
      rateHz,
      maxGapSeconds: Math.max(0.1, Number(options.maxGapSeconds) || DEFAULT_MAX_GAP_SECONDS),
      referenceBounds,
      candidateBounds,
    });
    if (paired.reference.length < minimumPairs) {
      return { ready: false, status: "learning", reason: "not enough gap-free pairs", pairs: paired.reference.length, overlapSeconds, referenceBounds, candidateBounds };
    }
    const reversals = slopeReversals(paired.reference);
    if (reversals < Math.max(2, Number(options.minimumReversals) || 4)) {
      return { ready: false, status: "learning", reason: "waiting for two full breaths", pairs: paired.reference.length, reversals, overlapSeconds, referenceBounds, candidateBounds };
    }
    const correlation = pearson(paired.reference, paired.candidate);
    const middle = Math.floor(paired.reference.length / 2);
    const firstHalf = pearson(paired.reference.slice(0, middle), paired.candidate.slice(0, middle));
    const secondHalf = pearson(paired.reference.slice(middle), paired.candidate.slice(middle));
    const minimumCorrelation = Math.max(0, Number(options.minimumCorrelation) || 0.55);
    const minimumHalfCorrelation = Math.max(0, Number(options.minimumHalfCorrelation) || 0.30);
    const provisionalCorrelation = Math.max(0, Number(options.provisionalCorrelation) || 0.18);
    const sign = correlation >= 0 ? 1 : -1;
    const consistent = [correlation, firstHalf, secondHalf].every(Number.isFinite)
      && Math.abs(correlation) >= minimumCorrelation
      && Math.abs(firstHalf) >= minimumHalfCorrelation
      && Math.abs(secondHalf) >= minimumHalfCorrelation
      && Math.sign(firstHalf) === sign
      && Math.sign(secondHalf) === sign;
    const provisional = !consistent
      && Number.isFinite(correlation)
      && Math.abs(correlation) >= provisionalCorrelation;
    return {
      ready: consistent,
      status: consistent ? "candidate" : provisional ? "provisional" : "uncertain",
      reason: consistent
        ? "stable zero-lag direction"
        : provisional
          ? "weak zero-lag Vernier match"
          : "direction agreement is weak or inconsistent",
      sign: consistent || provisional ? sign : null,
      correlation,
      firstHalf,
      secondHalf,
      pairs: paired.reference.length,
      reversals,
      overlapSeconds,
      referenceBounds,
      candidateBounds,
    };
  }

  window.PolarBreathingComparison = Object.freeze({
    alignmentEvidence,
    finiteOrderedPoints,
    pearson,
    resampleSeries,
    robustBounds,
  });
})();
