(() => {
  "use strict";

  const UUIDS = Object.freeze({
    heartRateService: "0000180d-0000-1000-8000-00805f9b34fb",
    heartRateMeasurement: "00002a37-0000-1000-8000-00805f9b34fb",
    batteryService: "0000180f-0000-1000-8000-00805f9b34fb",
    batteryLevel: "00002a19-0000-1000-8000-00805f9b34fb",
    pmdService: "fb005c80-02e7-f387-1cad-8acd2d8df0c8",
    pmdControl: "fb005c81-02e7-f387-1cad-8acd2d8df0c8",
    pmdData: "fb005c82-02e7-f387-1cad-8acd2d8df0c8",
  });
  const MEASUREMENTS = Object.freeze({ ecg: 0x00, accelerometer: 0x02 });
  const COMMANDS = Object.freeze({
    startEcg: Object.freeze([0x02, 0x00, 0x00, 0x01, 130, 0, 0x01, 0x01, 14, 0]),
    startAccelerometer: Object.freeze([0x02, 0x02, 0x02, 0x01, 8, 0, 0x00, 0x01, 200, 0, 0x01, 0x01, 16, 0]),
    stopEcg: Object.freeze([0x03, 0x00]),
    stopAccelerometer: Object.freeze([0x03, 0x02]),
  });
  const SAMPLE_RATE_HZ = 200;
  const GATT_CONNECT_RETRY_DELAY_MS = 300;

  class WebBluetoothError extends Error {
    constructor(code, message, retryable = false) {
      super(message);
      this.name = "WebBluetoothError";
      this.code = code;
      this.retryable = Boolean(retryable);
    }
  }

  function asBytes(value) {
    if (value instanceof Uint8Array) return value;
    if (value instanceof DataView) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    return Uint8Array.from(value || []);
  }

  function readUnsigned64Le(bytes, offset) {
    let result = 0n;
    for (let index = 7; index >= 0; index -= 1) {
      result = (result << 8n) | BigInt(bytes[offset + index]);
    }
    return result;
  }

  function readSigned16Le(bytes, offset) {
    const raw = bytes[offset] | (bytes[offset + 1] << 8);
    return raw & 0x8000 ? raw - 0x10000 : raw;
  }

  function readSigned24Le(bytes, offset) {
    const raw = bytes[offset] | (bytes[offset + 1] << 8) | (bytes[offset + 2] << 16);
    return raw & 0x800000 ? raw - 0x1000000 : raw;
  }

  function readSignedBits(bytes, bitOffset, width) {
    let value = 0;
    for (let shift = 0; shift < width; shift += 1) {
      const absolute = bitOffset + shift;
      value += ((bytes[Math.floor(absolute / 8)] >> (absolute % 8)) & 1) * (2 ** shift);
    }
    return value >= 2 ** (width - 1) ? value - 2 ** width : value;
  }

  function decodeCompressedAccelerometer(payload) {
    if (payload.length < 8) {
      throw new WebBluetoothError("PMD_INVALID_ACC", "Compressed accelerometer payload has an invalid length.");
    }
    let xMg = readSigned16Le(payload, 0);
    let yMg = readSigned16Le(payload, 2);
    let zMg = readSigned16Le(payload, 4);
    const samples = [{ xMg, yMg, zMg }];
    let offset = 6;
    while (offset < payload.length) {
      if (offset + 2 > payload.length) {
        throw new WebBluetoothError("PMD_INVALID_ACC", "Compressed accelerometer delta header is truncated.");
      }
      const deltaWidth = payload[offset];
      const sampleCount = payload[offset + 1];
      offset += 2;
      if (deltaWidth < 1 || deltaWidth > 16) {
        throw new WebBluetoothError("PMD_INVALID_ACC", `Unsupported accelerometer delta width: ${deltaWidth}.`);
      }
      const bitLength = sampleCount * deltaWidth * 3;
      const byteLength = Math.ceil(bitLength / 8);
      if (offset + byteLength > payload.length) {
        throw new WebBluetoothError("PMD_INVALID_ACC", "Compressed accelerometer delta block is truncated.");
      }
      const deltaBytes = payload.subarray(offset, offset + byteLength);
      let bitOffset = 0;
      for (let index = 0; index < sampleCount; index += 1) {
        xMg = Math.max(-32768, Math.min(32767, xMg + readSignedBits(deltaBytes, bitOffset, deltaWidth)));
        bitOffset += deltaWidth;
        yMg = Math.max(-32768, Math.min(32767, yMg + readSignedBits(deltaBytes, bitOffset, deltaWidth)));
        bitOffset += deltaWidth;
        zMg = Math.max(-32768, Math.min(32767, zMg + readSignedBits(deltaBytes, bitOffset, deltaWidth)));
        bitOffset += deltaWidth;
        samples.push({ xMg, yMg, zMg });
      }
      offset += byteLength;
    }
    return samples;
  }

  function decodeHeartRate(value) {
    const bytes = asBytes(value);
    if (bytes.length < 2) return { beatsPerMinute: 0, rrIntervalsMs: [] };
    const flags = bytes[0];
    const uses16BitHeartRate = Boolean(flags & 0x01);
    let cursor = uses16BitHeartRate ? 3 : 2;
    const beatsPerMinute = uses16BitHeartRate && bytes.length >= 3
      ? bytes[1] | (bytes[2] << 8)
      : bytes[1];
    if (flags & 0x08) cursor += 2;
    const rrIntervalsMs = [];
    if (flags & 0x10) {
      while (cursor + 1 < bytes.length) {
        rrIntervalsMs.push((bytes[cursor] | (bytes[cursor + 1] << 8)) * (1000 / 1024));
        cursor += 2;
      }
    }
    return { beatsPerMinute, rrIntervalsMs };
  }

  function decodePmd(value) {
    const bytes = asBytes(value);
    if (bytes.length < 10) {
      throw new WebBluetoothError("PMD_FRAME_TOO_SHORT", "PMD frame is shorter than its 10-byte header.");
    }
    const measurement = bytes[0];
    const sensorTimestampNs = readUnsigned64Le(bytes, 1).toString();
    const frameType = bytes[9];
    const payload = bytes.subarray(10);

    if (measurement === MEASUREMENTS.ecg && frameType === 0x00) {
      if (payload.length % 3 !== 0) {
        throw new WebBluetoothError("PMD_INVALID_ECG", "ECG payload is not a sequence of signed 24-bit samples.");
      }
      const microvolts = [];
      for (let offset = 0; offset < payload.length; offset += 3) {
        microvolts.push(readSigned24Le(payload, offset));
      }
      return { kind: "ecg", sensorTimestampNs, microvolts };
    }

    if (measurement !== MEASUREMENTS.accelerometer) {
      throw new WebBluetoothError("PMD_UNSUPPORTED_FRAME", "Unsupported PMD measurement or frame type.");
    }
    const compressed = Boolean(frameType & 0x80);
    const baseFrameType = frameType & 0x7f;
    let samples = [];
    if (!compressed && baseFrameType === 0x01) {
      if (payload.length % 6 !== 0) {
        throw new WebBluetoothError("PMD_INVALID_ACC", "Accelerometer payload has an invalid length.");
      }
      for (let offset = 0; offset < payload.length; offset += 6) {
        samples.push({
          xMg: readSigned16Le(payload, offset),
          yMg: readSigned16Le(payload, offset + 2),
          zMg: readSigned16Le(payload, offset + 4),
        });
      }
    } else if (compressed && (baseFrameType === 0x00 || baseFrameType === 0x01)) {
      samples = decodeCompressedAccelerometer(payload);
    } else {
      throw new WebBluetoothError("PMD_UNSUPPORTED_FRAME", "Unsupported PMD accelerometer frame type.");
    }
    return { kind: "accelerometer", sensorTimestampNs, samples };
  }

  function defaultBreathingSettings() {
    return {
      volumeMode: "timed-pca-v1",
      stateMode: "hysteresis-v1",
      axes: [true, false, true],
      calibrationWindowSeconds: 12,
      minimumAxisRangeG: 0.01,
      smoothingWindowSeconds: 0.75,
      volumeFilterTauSeconds: 0.18,
      sensitivity: 0.60,
      staleTimeoutSeconds: 0.50,
      invertDirection: false,
      adaptiveBounds: false,
      adaptiveWindowSeconds: 20,
      lowerQuantile: 0.05,
      upperQuantile: 0.95,
      phaseDerivativeTauSeconds: 0.40,
      phaseEnterThresholdPerSecond: 0.030,
      phaseHoldThresholdPerSecond: 0.025,
      phaseConfirmationSeconds: 0.40,
      phaseMinimumDwellSeconds: 0.40,
    };
  }

  function finiteClamped(value, fallback, low, high) {
    const number = Number(value);
    return Math.max(low, Math.min(high, Number.isFinite(number) ? number : fallback));
  }

  function clampBreathingSettings(value = {}) {
    const settings = { ...defaultBreathingSettings(), ...value };
    const hasExplicitSettings = value && Object.keys(value).length > 0;
    settings.volumeMode = value.volumeMode || (value.algorithm === "legacy-v0" ? "legacy-v0" : (hasExplicitSettings ? "legacy-v0" : settings.volumeMode));
    settings.stateMode = value.stateMode || (value.phaseAlgorithm === "legacy-v0" ? "legacy-v0" : (hasExplicitSettings ? "legacy-v0" : settings.stateMode));
    settings.volumeMode = settings.volumeMode === "legacy-v0" ? "legacy-v0" : "timed-pca-v1";
    settings.stateMode = settings.stateMode === "legacy-v0" ? "legacy-v0" : "hysteresis-v1";
    if (settings.volumeMode === "legacy-v0") settings.stateMode = "legacy-v0";
    if (settings.volumeMode === "legacy-v0" && settings.stateMode === "legacy-v0"
      && hasExplicitSettings && !Object.hasOwn(value, "adaptiveBounds")) settings.adaptiveBounds = true;
    const axes = Array.isArray(settings.axes) ? settings.axes.slice(0, 3).map(Boolean) : [true, false, true];
    while (axes.length < 3) axes.push(false);
    settings.axes = axes.filter(Boolean).length >= 2 ? axes : [true, false, true];
    settings.calibrationWindowSeconds = finiteClamped(settings.calibrationWindowSeconds, 12, 1, 60);
    settings.minimumAxisRangeG = finiteClamped(settings.minimumAxisRangeG, 0.01, 0.001, 0.25);
    settings.smoothingWindowSeconds = finiteClamped(settings.smoothingWindowSeconds, 0.75, 0.05, 5);
    settings.volumeFilterTauSeconds = finiteClamped(settings.volumeFilterTauSeconds, 0.18, 0.01, 5);
    settings.sensitivity = finiteClamped(settings.sensitivity, 0.60, 0, 1);
    settings.staleTimeoutSeconds = finiteClamped(settings.staleTimeoutSeconds, 0.50, 0.25, 30);
    settings.phaseDerivativeTauSeconds = finiteClamped(settings.phaseDerivativeTauSeconds, 0.40, 0.01, 5);
    settings.phaseEnterThresholdPerSecond = finiteClamped(settings.phaseEnterThresholdPerSecond, 0.030, 0.001, 5);
    settings.phaseHoldThresholdPerSecond = finiteClamped(settings.phaseHoldThresholdPerSecond, 0.025, 0, 5);
    settings.phaseConfirmationSeconds = finiteClamped(settings.phaseConfirmationSeconds, 0.40, 0, 5);
    settings.phaseMinimumDwellSeconds = finiteClamped(settings.phaseMinimumDwellSeconds, 0.40, 0, 5);
    settings.phaseHoldThresholdPerSecond = Math.min(settings.phaseHoldThresholdPerSecond, settings.phaseEnterThresholdPerSecond);
    settings.adaptiveWindowSeconds = finiteClamped(settings.adaptiveWindowSeconds, 20, 5, 300);
    settings.lowerQuantile = finiteClamped(settings.lowerQuantile, 0.05, 0, 0.40);
    settings.upperQuantile = finiteClamped(settings.upperQuantile, 0.95, 0.60, 1);
    if (settings.upperQuantile - settings.lowerQuantile < 0.10) {
      settings.lowerQuantile = 0.05;
      settings.upperQuantile = 0.95;
    }
    settings.invertDirection = Boolean(settings.invertDirection);
    settings.adaptiveBounds = settings.adaptiveBounds !== false;
    return settings;
  }

  function dot(left, right) {
    return left[0] * right[0] + left[1] * right[1] + left[2] * right[2];
  }

  function subtract(left, right) {
    return [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
  }

  function quantile(sorted, fraction) {
    const position = (sorted.length - 1) * fraction;
    const low = Math.floor(position);
    const high = Math.ceil(position);
    return sorted[low] + (sorted[high] - sorted[low]) * (position - low);
  }

  function inverseLerp(low, high, value) {
    if (Math.abs(high - low) < 1e-8) return 0.5;
    return Math.max(0, Math.min(1, (value - low) / (high - low)));
  }

  class BreathingProcessor {
    constructor(settings = {}) {
      this.reset(settings);
    }

    reset(settings = this.settings) {
      this.settings = clampBreathingSettings(settings);
      this.filtered = [0, 0, 0];
      this.hasFiltered = false;
      this.calibration = [];
      this.lastCalibrationAttemptSeconds = Number.NEGATIVE_INFINITY;
      this.center = [0, 0, 0];
      this.baseline = [0, 0, 0];
      this.axis = [0, 1, 0];
      this.boundMin = -0.02;
      this.boundMax = 0.02;
      this.calibrationSpan = 0.04;
      this.calibrated = false;
      this.projection = 0;
      this.adaptiveProjections = [];
      this.lastAdaptiveUpdateSeconds = 0;
      this.lastEmittedVolume = 0.5;
      this.hasEmittedVolume = false;
      this.elapsedSeconds = 0;
      this.lastPushAt = null;
      this.motionFiltered = [0, 0, 0];
      this.hasMotionFiltered = false;
      this.motionDeltaEmaG = 0;
    }

    applySettings(settings) {
      const next = clampBreathingSettings(settings);
      if (JSON.stringify(next) !== JSON.stringify(this.settings)) this.reset(next);
    }

    calibrationTarget() {
      return Math.round(this.settings.calibrationWindowSeconds * SAMPLE_RATE_HZ);
    }

    smoothingAlpha() {
      return Math.max(0.001, Math.min(1, 2 / (this.settings.smoothingWindowSeconds * SAMPLE_RATE_HZ + 1)));
    }

    phaseVelocityThresholdPerSecond() {
      return (0.0005 + ((1 - this.settings.sensitivity) ** 2) * 0.015625) / 0.05;
    }

    classifyPhaseDelta(normalizedDelta, batchDurationSeconds) {
      const duration = Math.max(1 / SAMPLE_RATE_HZ, batchDurationSeconds);
      const velocity = normalizedDelta / duration;
      const threshold = this.phaseVelocityThresholdPerSecond();
      if (velocity > threshold) return 1;
      if (velocity < -threshold) return -1;
      return 0;
    }

    push(samples, nowMilliseconds = performance.now()) {
      if (!Array.isArray(samples) || !samples.length) return null;
      const stale = this.lastPushAt !== null
        && nowMilliseconds - this.lastPushAt > this.settings.staleTimeoutSeconds * 1000;
      this.lastPushAt = nowMilliseconds;
      let latestVolume = this.lastEmittedVolume;
      const alpha = this.smoothingAlpha();

      for (const sample of samples) {
        const raw = [sample.xMg / 1000, sample.yMg / 1000, sample.zMg / 1000];
        if (!this.hasMotionFiltered) {
          this.motionFiltered = [...raw];
          this.hasMotionFiltered = true;
        } else {
          const previous = [...this.motionFiltered];
          this.motionFiltered = this.motionFiltered
            .map((axis, index) => axis + (raw[index] - axis) * alpha);
          const delta = subtract(this.motionFiltered, previous);
          const magnitude = Math.sqrt(dot(delta, delta));
          this.motionDeltaEmaG += (magnitude - this.motionDeltaEmaG) * 0.01;
        }
        const current = raw
          .map((axis, index) => this.settings.axes[index] ? axis : 0);
        if (!this.hasFiltered) {
          this.filtered = current;
          this.hasFiltered = true;
        } else {
          this.filtered = this.filtered.map((axis, index) => axis + (current[index] - axis) * alpha);
        }
        this.elapsedSeconds += 1 / SAMPLE_RATE_HZ;
        if (!this.calibrated) {
          this.calibration.push([...this.filtered]);
          const target = this.calibrationTarget();
          if (this.calibration.length > target) this.calibration.splice(0, this.calibration.length - target);
          if (this.calibration.length === target
            && this.elapsedSeconds - this.lastCalibrationAttemptSeconds >= 0.5) {
            this.lastCalibrationAttemptSeconds = this.elapsedSeconds;
            this.tryCalibrate();
          }
        }
        if (this.calibrated) {
          const baselineAlpha = 1 / (SAMPLE_RATE_HZ * 10);
          this.baseline = this.baseline.map((axis, index) => (
            axis + (this.filtered[index] - axis) * baselineAlpha
          ));
          this.projection = dot(subtract(this.filtered, this.baseline), this.axis);
          this.updateAdaptiveBounds();
          latestVolume = inverseLerp(this.boundMin, this.boundMax, this.projection);
        }
      }

      const motionScore = this.motionScore();
      const ready = this.calibrated && !stale && motionScore >= 0.35;
      const confidence = ready ? this.signalConfidence(motionScore) : 0;
      let phase = 0;
      if (ready) {
        if (!this.hasEmittedVolume) {
          this.hasEmittedVolume = true;
        } else {
          const delta = latestVolume - this.lastEmittedVolume;
          phase = this.classifyPhaseDelta(delta, samples.length / SAMPLE_RATE_HZ);
        }
      }
      this.lastEmittedVolume = latestVolume;
      const values = [
        { id: "breathing_calibration", value: Math.max(0, Math.min(1, this.calibration.length / this.calibrationTarget())) },
        { id: "breathing_phase", value: phase },
        { id: "breathing_signal_confidence", value: confidence },
        { id: "breathing_signal_ready", value: ready ? 1 : 0 },
      ];
      if (this.calibrated) {
        values.push(
          { id: "acc_breathing_magnitude", value: this.projection },
          { id: "breathing_volume", value: latestVolume },
          { id: "breathing_axis_range", value: this.boundMax - this.boundMin },
        );
      }
      return {
        calibrated: this.calibrated,
        ready,
        confidence01: confidence,
        phase,
        magnitudeG: this.projection,
        volume01: latestVolume,
        axisRangeG: this.boundMax - this.boundMin,
        timeSeconds: this.elapsedSeconds,
        values,
      };
    }

    tryCalibrate() {
      const count = this.calibration.length;
      const center = [0, 0, 0];
      for (const sample of this.calibration) {
        for (let index = 0; index < 3; index += 1) center[index] += sample[index] / count;
      }
      const covariance = Array.from({ length: 3 }, () => [0, 0, 0]);
      for (const sample of this.calibration) {
        const delta = subtract(sample, center);
        for (let row = 0; row < 3; row += 1) {
          for (let column = 0; column < 3; column += 1) {
            covariance[row][column] += delta[row] * delta[column] / count;
          }
        }
      }
      let dominantDimension = 0;
      for (let index = 1; index < 3; index += 1) {
        if (covariance[index][index] > covariance[dominantDimension][dominantDimension]) dominantDimension = index;
      }
      let axis = [0, 0, 0];
      axis[dominantDimension] = 1;
      for (let iteration = 0; iteration < 8; iteration += 1) {
        const next = covariance.map((row) => dot(row, axis));
        const magnitude = Math.sqrt(dot(next, next));
        if (magnitude < 1e-8) return;
        axis = next.map((value) => value / magnitude);
      }
      if (this.settings.invertDirection) axis = axis.map((value) => -value);
      const projections = this.calibration
        .map((sample) => dot(subtract(sample, center), axis))
        .sort((left, right) => left - right);
      let low = quantile(projections, this.settings.lowerQuantile);
      let high = quantile(projections, this.settings.upperQuantile);
      const rawRange = high - low;
      if (rawRange < this.settings.minimumAxisRangeG) return;
      const ease = rawRange * 0.03;
      low += ease;
      high -= ease;
      if (high <= low) return;
      this.center = center;
      this.baseline = [...center];
      this.axis = axis;
      this.boundMin = low;
      this.boundMax = high;
      this.calibrationSpan = high - low;
      this.calibrated = true;
      this.adaptiveProjections = [];
    }

    updateAdaptiveBounds() {
      const last = this.adaptiveProjections[this.adaptiveProjections.length - 1];
      if (last && this.elapsedSeconds - last.time < 0.05) return;
      this.adaptiveProjections.push({ time: this.elapsedSeconds, value: this.projection });
      const cutoff = this.elapsedSeconds - this.settings.adaptiveWindowSeconds;
      while (this.adaptiveProjections.length && this.adaptiveProjections[0].time < cutoff) {
        this.adaptiveProjections.shift();
      }
      if (!this.settings.adaptiveBounds) return;
      if (this.elapsedSeconds - this.lastAdaptiveUpdateSeconds < 0.5
        || this.adaptiveProjections.length < 80) return;
      const elapsedSinceAdaptive = this.lastAdaptiveUpdateSeconds === 0
        ? 0.5 : this.elapsedSeconds - this.lastAdaptiveUpdateSeconds;
      this.lastAdaptiveUpdateSeconds = this.elapsedSeconds;
      const values = this.adaptiveProjections.map((entry) => entry.value).sort((left, right) => left - right);
      const low = quantile(values, this.settings.lowerQuantile);
      const high = quantile(values, this.settings.upperQuantile);
      const span = high - low;
      if (span < this.settings.minimumAxisRangeG
        || span < this.calibrationSpan * 0.5
        || span > this.calibrationSpan * 2) return;
      const dt = Math.max(0.5, elapsedSinceAdaptive);
      const alpha = 1 - Math.exp(-0.5 * dt);
      this.boundMin += (low - this.boundMin) * alpha;
      this.boundMax += (high - this.boundMax) * alpha;
    }

    motionScore() {
      const threshold = Math.max(this.settings.minimumAxisRangeG * 0.1, 0.001);
      const ratio = this.motionDeltaEmaG / threshold;
      return Math.max(0, Math.min(1, 1 / (1 + ratio * ratio)));
    }

    signalConfidence(motionScore) {
      const rangeScore = Math.max(0, Math.min(1,
        (this.boundMax - this.boundMin) / (this.settings.minimumAxisRangeG * 2)));
      const coverage = Math.max(0, Math.min(1, this.adaptiveProjections.length / (SAMPLE_RATE_HZ * 0.8)));
      return Math.max(0, Math.min(1,
        rangeScore * motionScore * (0.4 + 0.6 * coverage * this.periodicityScore())));
    }

    periodicityScore() {
      if (this.adaptiveProjections.length < 80) return 0;
      const samples = this.adaptiveProjections.map((entry) => entry.value);
      const mean = samples.reduce((sum, value) => sum + value, 0) / samples.length;
      const centered = samples.map((value) => value - mean);
      const maximumLag = Math.min(250, centered.length - 40);
      let best = 0;
      for (let lag = 29; lag <= maximumLag; lag += 1) {
        let covariance = 0;
        let leftEnergy = 0;
        let rightEnergy = 0;
        for (let index = lag; index < centered.length; index += 1) {
          const left = centered[index - lag];
          const right = centered[index];
          covariance += left * right;
          leftEnergy += left * left;
          rightEnergy += right * right;
        }
        const denominator = Math.sqrt(leftEnergy * rightEnergy);
        if (denominator > 1e-12) best = Math.max(best, Math.max(0, Math.min(1, covariance / denominator)));
      }
      return best;
    }
  }

  // Source-time processor used by the live browser adapter.  PMD timestamps
  // identify the newest sample in a notification; notification arrival time
  // is deliberately never used for signal timing.
  class TimedBreathingProcessor {
    constructor(settings = {}) { this.reset(settings); }

    reset(settings = this.settings) {
      this.settings = clampBreathingSettings({ ...defaultBreathingSettings(), ...settings });
      this.samplesSeen = 0;
      this.calibration = [];
      this.center = [0, 0, 0];
      this.axis = [1, 0, 0];
      this.boundMin = -0.02;
      this.boundMax = 0.02;
      this.calibrationSpan = 0.04;
      this.pcaDominance01 = 0;
      this.calibrated = false;
      this.filtered = null;
      this.motionFiltered = null;
      this.motionDeltaEmaG = 0;
      this.lastSourceNs = null;
      this.lastAnchorNs = null;
      this.lastProcessedNs = null;
      this.watermarkNs = null;
      this.lastVolume = 0.5;
      this.lastProjection = null;
      this.derivative = 0;
      this.phase = 0;
      this.activeSinceNs = null;
      this.candidate = 0;
      this.candidateSinceNs = null;
      this.adaptiveProjections = [];
      this.lastAdaptiveUpdateNs = null;
      this.lost = false;
      this.diagnostics = { accepted: 0, lateDropped: 0, forwardGaps: 0, resets: 0 };
    }

    applySettings(settings) {
      const next = clampBreathingSettings(settings);
      if (JSON.stringify(next) !== JSON.stringify(this.settings)) this.reset(next);
    }

    alpha(dt, tau) { return Math.max(0, Math.min(1, dt / Math.max(1e-6, tau + dt))); }

    sourceTimes(samples, newest, interpolate = true) {
      const end = BigInt(newest);
      if (interpolate && this.lastAnchorNs !== null && end > this.lastAnchorNs) {
        const anchorDelta = end - this.lastAnchorNs;
        // PMD's newest timestamp is the anchor for the whole notification.
        // Interpolating the interval between anchors avoids a boundary-sized
        // 2–3 ms timing kink on real H10 batches (typically 36 samples). Gap
        // eligibility is decided from nominalFirst by pushTimed, not here.
        const count = BigInt(samples.length);
        return samples.map((_, index) => this.lastAnchorNs
          + (anchorDelta * BigInt(index + 1)) / count);
      }
      const step = 1_000_000_000n / BigInt(SAMPLE_RATE_HZ);
      return samples.map((_, index) => end - BigInt(samples.length - 1 - index) * step);
    }

    pca(samples) {
      const count = samples.length;
      const center = [0, 0, 0];
      for (const sample of samples) for (let i = 0; i < 3; i += 1) center[i] += sample[i] / count;
      const covariance = Array.from({ length: 3 }, () => [0, 0, 0]);
      for (const sample of samples) {
        const delta = subtract(sample, center);
        for (let i = 0; i < 3; i += 1) for (let j = 0; j < 3; j += 1) covariance[i][j] += delta[i] * delta[j] / count;
      }
      let dominantDimension = 0;
      for (let i = 1; i < 3; i += 1) if (covariance[i][i] > covariance[dominantDimension][dominantDimension]) dominantDimension = i;
      let axis = [0, 0, 0];
      axis[dominantDimension] = 1;
      for (let iteration = 0; iteration < 32; iteration += 1) {
        const next = covariance.map((row) => dot(row, axis));
        const magnitude = Math.sqrt(dot(next, next));
        if (magnitude < 1e-10) return null;
        axis = next.map((value) => value / magnitude);
      }
      const signIndex = axis.reduce((best, value, index) => Math.abs(value) > Math.abs(axis[best]) ? index : best, 0);
      if (axis[signIndex] < 0) axis = axis.map((value) => -value);
      if (this.settings.invertDirection) axis = axis.map((value) => -value);
      const trace = covariance[0][0] + covariance[1][1] + covariance[2][2];
      const eigenvalue = dot(axis, covariance.map((row) => dot(row, axis)));
      const projections = samples.map((sample) => dot(subtract(sample, center), axis)).sort((a, b) => a - b);
      const low = quantile(projections, this.settings.lowerQuantile);
      const high = quantile(projections, this.settings.upperQuantile);
      const dominance = trace > 1e-10 ? Math.max(0, Math.min(1, eigenvalue / trace)) : 0;
      if (!Number.isFinite(low) || high - low < this.settings.minimumAxisRangeG || dominance < 0.05) return null;
      return { center, axis, low, high, dominance };
    }

    classifyDerivative(derivative, timeNs) {
      const enter = Number(this.settings.phaseEnterThresholdPerSecond);
      const hold = Number(this.settings.phaseHoldThresholdPerSecond);
      let requested = this.phase;
      if (derivative >= enter) requested = 1;
      else if (derivative <= -enter) requested = -1;
      else if (Math.abs(derivative) <= hold) requested = 0;
      if (requested === this.phase) { this.candidate = requested; this.candidateSinceNs = null; return; }
      if (this.candidate !== requested || this.candidateSinceNs === null) {
        this.candidate = requested;
        this.candidateSinceNs = timeNs;
        return;
      }
      const confirmed = Number(timeNs - this.candidateSinceNs) / 1e9 >= Number(this.settings.phaseConfirmationSeconds);
      const dwell = this.activeSinceNs === null
        || Number(timeNs - this.activeSinceNs) / 1e9 >= Number(this.settings.phaseMinimumDwellSeconds);
      if (confirmed && dwell) {
        this.phase = requested;
        this.activeSinceNs = timeNs;
        this.candidateSinceNs = null;
      }
    }

    updateAdaptiveBounds(timeNs, projection) {
      const last = this.adaptiveProjections.at(-1);
      if (!last || timeNs - last.timeNs >= 50_000_000n) {
        this.adaptiveProjections.push({ timeNs, projection });
      }
      const cutoff = timeNs - BigInt(Math.round(Number(this.settings.adaptiveWindowSeconds) * 1e9));
      while (this.adaptiveProjections.length && this.adaptiveProjections[0].timeNs < cutoff) {
        this.adaptiveProjections.shift();
      }
      if (!this.settings.adaptiveBounds || this.adaptiveProjections.length < 80) return;
      const elapsedNs = this.lastAdaptiveUpdateNs === null ? 500_000_000n : timeNs - this.lastAdaptiveUpdateNs;
      if (elapsedNs < 500_000_000n) return;
      this.lastAdaptiveUpdateNs = timeNs;
      const projections = this.adaptiveProjections.map((entry) => entry.projection).sort((left, right) => left - right);
      const lower = quantile(projections, this.settings.lowerQuantile);
      const upper = quantile(projections, this.settings.upperQuantile);
      const span = upper - lower;
      if (span < this.settings.minimumAxisRangeG
        || span < this.calibrationSpan * 0.50
        || span > this.calibrationSpan * 2) return;
      const alpha = 1 - Math.exp(-0.50 * (Number(elapsedNs) / 1e9));
      this.boundMin += (lower - this.boundMin) * alpha;
      this.boundMax += (upper - this.boundMax) * alpha;
    }

    pushTimed(samples, sensorTimestampNs) {
      if (!Array.isArray(samples) || samples.length === 0 || sensorTimestampNs == null) return null;
      const times = this.sourceTimes(samples, sensorTimestampNs);
      const newest = times[times.length - 1];
      const nominalPeriodNs = 1_000_000_000n / BigInt(SAMPLE_RATE_HZ);
      const nominalFirst = BigInt(sensorTimestampNs) - nominalPeriodNs * BigInt(samples.length - 1);
      const boundaryGapNs = this.lastSourceNs === null ? 0n : nominalFirst - this.lastSourceNs;
      const staleNs = BigInt(Math.round(Number(this.settings.staleTimeoutSeconds) * 1e9));
      const boundaryForward = this.lastSourceNs !== null && nominalFirst > this.lastSourceNs;
      const boundaryGap = boundaryForward && boundaryGapNs > staleNs;
      const interpolate = this.lastSourceNs === null || (boundaryForward && !boundaryGap);
      const anchoredTimes = this.sourceTimes(samples, sensorTimestampNs, interpolate);
      for (let index = 0; index < times.length; index += 1) times[index] = anchoredTimes[index];
      if (this.watermarkNs !== null && newest < this.watermarkNs - 250_000_000n) {
        const resetCount = this.diagnostics.resets + 1;
        this.reset(this.settings);
        this.diagnostics.resets = resetCount;
      }
      if (this.watermarkNs !== null && newest <= this.watermarkNs) {
        this.diagnostics.lateDropped += samples.length;
        return this.snapshot(newest, false);
      }
      let hadForwardGap = false;
      if (this.lastSourceNs !== null) {
        const gap = Number(times[0] - this.lastSourceNs) / 1e9;
        if (gap < -0.25) {
          const resetCount = this.diagnostics.resets + 1;
          this.reset(this.settings);
          this.diagnostics.resets = resetCount;
        } else if (boundaryGap) {
          this.lost = true;
          hadForwardGap = true;
          this.phase = 0;
          this.activeSinceNs = null;
          this.candidateSinceNs = null;
          this.lastProjection = null;
          this.derivative = 0;
          this.diagnostics.forwardGaps += 1;
        }
      }
      const accepted = [];
      const presentationPoints = [];
      for (let index = 0; index < samples.length; index += 1) {
        const timeNs = times[index];
        if (this.watermarkNs !== null && timeNs <= this.watermarkNs) { this.diagnostics.lateDropped += 1; continue; }
        accepted.push({ values: [samples[index].xMg / 1000, samples[index].yMg / 1000, samples[index].zMg / 1000], timeNs });
      }
      if (!accepted.length) return this.snapshot(newest, false);
      this.watermarkNs = accepted[accepted.length - 1].timeNs;
      this.lastSourceNs = this.watermarkNs;
      this.lastAnchorNs = newest;
      this.lost = hadForwardGap;
      for (const entry of accepted) {
        const current = entry.values.map((v, i) => this.settings.axes[i] ? v : 0);
        const dt = this.lastProcessedNs == null ? 1 / SAMPLE_RATE_HZ : Math.max(1e-6, Number(entry.timeNs - this.lastProcessedNs) / 1e9);
        const a = this.alpha(dt, Number(this.settings.volumeFilterTauSeconds));
        if (this.motionFiltered === null) this.motionFiltered = [...entry.values];
        else {
          const previousMotion = [...this.motionFiltered];
          this.motionFiltered = this.motionFiltered.map((v, i) => v + (entry.values[i] - v) * a);
          const delta = subtract(this.motionFiltered, previousMotion);
          const motionAlpha = this.alpha(dt, 0.50);
          this.motionDeltaEmaG += motionAlpha * (Math.sqrt(dot(delta, delta)) - this.motionDeltaEmaG);
        }
        this.filtered = this.filtered === null ? current : this.filtered.map((v, i) => v + (current[i] - v) * a);
        this.lastProcessedNs = entry.timeNs;
        this.samplesSeen += 1;
        if (!this.calibrated) {
          this.calibration.push({ timeNs: entry.timeNs, value: [...this.filtered] });
          const calibrationWindowNs = BigInt(Math.round(Number(this.settings.calibrationWindowSeconds) * 1e9));
          const retainNs = calibrationWindowNs + 10_000_000n;
          while (this.calibration.length && entry.timeNs - this.calibration[0].timeNs > retainNs) this.calibration.shift();
          const calibrationReady = this.calibration.length >= 8
            && entry.timeNs - this.calibration[0].timeNs >= calibrationWindowNs;
          if (calibrationReady) {
            const result = this.pca(this.calibration.map((sample) => sample.value));
            if (result) {
              this.center = result.center;
              this.axis = result.axis;
              this.boundMin = result.low;
              this.boundMax = result.high;
              this.calibrationSpan = Math.max(1e-8, result.high - result.low);
              this.pcaDominance01 = result.dominance;
              this.calibrated = true;
              const initialProjection = dot(subtract(this.filtered, this.center), this.axis);
              this.lastProjection = initialProjection;
              this.activeSinceNs = entry.timeNs;
            }
          }
        }
        if (this.calibrated) {
          const projection = dot(subtract(this.filtered, this.center), this.axis);
          const span = Math.max(1e-8, this.calibrationSpan);
          this.updateAdaptiveBounds(entry.timeNs, projection);
          const volume = inverseLerp(this.boundMin, this.boundMax, projection);
          if (!hadForwardGap && this.lastProjection !== null) {
            const rawDerivative = (projection - this.lastProjection) / span / dt;
            this.derivative += this.alpha(dt, Number(this.settings.phaseDerivativeTauSeconds)) * (rawDerivative - this.derivative);
            this.classifyDerivative(this.derivative, entry.timeNs);
          }
          if (!hadForwardGap) this.lastProjection = projection;
          this.lastVolume = volume;
          presentationPoints.push({ sourceTimestampNs: String(entry.timeNs), volume01: volume });
        }
      }
      this.diagnostics.accepted += accepted.length;
      return this.snapshot(newest, true, presentationPoints);
    }

    snapshot(timeNs, accepted, presentationPoints = []) {
      const motionThreshold = Math.max(Number(this.settings.minimumAxisRangeG) * 0.1, 0.001);
      const motionRatio = this.motionDeltaEmaG / motionThreshold;
      const motionScore = Math.max(0, Math.min(1, 1 / (1 + motionRatio * motionRatio)));
      const ready = this.calibrated && !this.lost && motionScore >= 0.35;
      const rangeScore = Math.max(0, Math.min(1, this.calibrationSpan / (Number(this.settings.minimumAxisRangeG) * 2)));
      const confidence = ready ? Math.max(0, Math.min(1, rangeScore * motionScore * this.pcaDominance01)) : 0;
      const calibration = this.calibrated ? 1 : (this.calibration.length < 2 ? 0
        : Math.max(0, Math.min(1, Number(this.calibration.at(-1).timeNs - this.calibration[0].timeNs)
          / (Number(this.settings.calibrationWindowSeconds) * 1e9))));
      const values = [
        { id: "breathing_calibration", value: calibration },
        { id: "breathing_phase", value: ready ? this.phase : 0 },
        { id: "breathing_signal_confidence", value: confidence },
        { id: "breathing_signal_ready", value: ready ? 1 : 0 },
      ];
      if (this.calibrated) values.push(
        { id: "acc_breathing_magnitude", value: this.lastProjection ?? 0 },
        { id: "breathing_volume", value: this.lastVolume },
        { id: "breathing_axis_range", value: this.boundMax - this.boundMin },
      );
      return { calibrated: this.calibrated, ready, lost: this.lost, accepted, phase: ready ? this.phase : 0, volume01: this.lastVolume, magnitudeG: this.lastProjection ?? 0, derivativePerSecond: this.derivative, sensorTimestampNs: String(timeNs), presentationPoints: presentationPoints.slice(-512), diagnostics: { ...this.diagnostics, motionScore, pcaDominance01: this.pcaDominance01, confidence01: confidence }, values };
    }
  }

  class BreathingPresentation {
    constructor(mode = "fresh-smooth", options = {}) { this.reset(mode === "fresh+smoothing" ? "fresh-smooth" : mode, options); }
    reset(mode = this.mode, options = this.options) { this.mode = mode; this.options = { delaySeconds: 0.18, smoothingSeconds: 0.12, ...options }; this.points = []; this.value = null; }
    push(point) {
      const t = Number(point.sensorTimestampNs) / 1e9;
      if (!Number.isFinite(t) || !Number.isFinite(point.volume01)) return null;
      this.points.push({ t, v: Math.max(0, Math.min(1, point.volume01)) });
      while (this.points.length > 512) this.points.shift();
      if (this.mode === "timestamp-faithful") {
        const target = t - Math.max(0, Number(this.options.delaySeconds));
        while (this.points.length > 2 && this.points[1].t <= target) this.points.shift();
        if (this.points.length < 2) return this.value;
        const left = this.points[0]; const right = this.points[1];
        const ratio = (target - left.t) / Math.max(1e-9, right.t - left.t);
        this.value = Math.max(0, Math.min(1, left.v + (right.v - left.v) * Math.max(0, Math.min(1, ratio))));
      } else {
        const dt = this.value === null ? 0 : Math.max(0, t - (this.lastT || t));
        const a = dt <= 0 ? 1 : dt / (Number(this.options.smoothingSeconds) + dt);
        this.value = this.value === null ? this.points.at(-1).v : this.value + a * (this.points.at(-1).v - this.value);
      }
      this.lastT = t;
      return this.value;
    }
  }

  function unavailableMessage(blocked = false) {
    const prefix = blocked
      ? "This browser blocks Web Bluetooth."
      : "Web Bluetooth is unavailable in this browser.";
    if (/Android/i.test(navigator.userAgent)) {
      return `${prefix} Open this site in Google Chrome on Android.`;
    }
    if (/Linux/i.test(navigator.userAgentData?.platform || navigator.platform || navigator.userAgent)) {
      return `${prefix} On Linux, use Chrome or Chromium with Experimental Web Platform features enabled.`;
    }
    return `${prefix} Use a Chrome or Edge browser with Web Bluetooth support.`;
  }

  function supportStatus() {
    if (!window.isSecureContext) {
      return { supported: false, reason: "Web Bluetooth requires HTTPS or localhost." };
    }
    if (typeof navigator.bluetooth?.requestDevice !== "function") {
      return { supported: false, reason: unavailableMessage() };
    }
    return { supported: true, reason: "Chromium Web Bluetooth · experimental" };
  }

  function browserBlocksBluetooth(error) {
    const browserMessage = String(error?.message || "");
    return error?.name === "NotSupportedError"
      || /globally disabled|web bluetooth (?:is )?not supported|permission (?:has been |is )?blocked/i.test(browserMessage);
  }

  function normalizeChooserError(error) {
    if (error instanceof WebBluetoothError) return error;
    if (browserBlocksBluetooth(error)) {
      return new WebBluetoothError(
        "WEB_BLUETOOTH_DISABLED",
        unavailableMessage(true),
        true,
      );
    }
    if (error?.name === "NotFoundError") {
      return new WebBluetoothError("BLUETOOTH_CHOOSER_CANCELLED", "No Polar H10 was selected.", true);
    }
    return normalizeBrowserError(error);
  }

  function normalizeBrowserError(error) {
    if (error instanceof WebBluetoothError) return error;
    if (browserBlocksBluetooth(error)) {
      return new WebBluetoothError("WEB_BLUETOOTH_DISABLED", unavailableMessage(true), true);
    }
    const browserMessage = String(error?.message || "");
    if (error?.name === "SecurityError" || error?.name === "NotAllowedError") {
      if (/permissions? policy|feature policy|not allowed to use (?:web )?bluetooth/i.test(browserMessage)) {
        return new WebBluetoothError(
          "WEB_BLUETOOTH_POLICY_BLOCKED",
          "This page's embedding policy blocks Web Bluetooth. Open Polar Stream directly in a top-level tab.",
          true,
        );
      }
      return new WebBluetoothError("BLUETOOTH_PERMISSION_DENIED", "Bluetooth permission was not granted.", true);
    }
    if (error?.name === "InvalidStateError") {
      return new WebBluetoothError(
        "BLUETOOTH_ADAPTER_UNAVAILABLE",
        "The Bluetooth adapter is not ready. Turn Bluetooth on, then try again.",
        true,
      );
    }
    if (error?.name === "NetworkError" || error?.name === "AbortError") {
      return new WebBluetoothError("BLUETOOTH_CONNECTION_FAILED", "The H10 Bluetooth connection failed.", true);
    }
    if (error?.name === "NotFoundError") {
      return new WebBluetoothError(
        "POLAR_SERVICE_UNAVAILABLE",
        "The selected device did not expose a required Polar H10 service.",
        true,
      );
    }
    return new WebBluetoothError(
      "BROWSER_BLE_FAILED",
      error?.message || "The browser could not connect to the Polar H10.",
      true,
    );
  }

  class PolarWebBluetoothSession {
    constructor() {
      this.device = null;
      this.server = null;
      this.control = null;
      this.pmdData = null;
      this.heartRate = null;
      this.onEvent = null;
      this.disconnecting = false;
      this.connected = false;
      this.wakeLock = null;
      this.wakeLockWanted = false;
      this.outputs = new Set(["raw_ecg", "raw_acc"]);
      this.breathing = new TimedBreathingProcessor();
      this.boundPmd = (event) => this.handlePmd(event);
      this.boundHeartRate = (event) => this.handleHeartRate(event);
      this.boundControl = (event) => this.handleControl(event);
      this.boundDisconnected = () => this.handleDisconnected();
      this.boundVisibility = () => {
        if (this.wakeLockWanted && document.visibilityState === "visible") {
          void this.requestWakeLock();
        }
      };
      document.addEventListener("visibilitychange", this.boundVisibility);
    }

    updateConfig(config = {}) {
      this.outputs = new Set(config.outputs || []);
      const candidates = [
        "breathing_volume", "breathing_signal_confidence", "breathing_signal_ready",
        "breathing_phase", "acc_breathing_magnitude", "breathing_calibration", "breathing_axis_range",
      ];
      let settings = defaultBreathingSettings();
      for (const id of candidates) {
        const processing = config.metricOptions?.[id]?.processing || {};
        if (processing.breathing || processing.breathingPhase) {
          settings = processing.breathing || processing.breathingPhase;
          break;
        }
      }
      const configured = createConfiguredBreathingProcessor(settings);
      if (this.breathing.constructor !== configured.constructor) this.breathing = configured;
      else this.breathing.applySettings(settings);
    }

    async connect(onEvent, config = {}) {
      const status = supportStatus();
      if (!status.supported) throw new WebBluetoothError("WEB_BLUETOOTH_UNAVAILABLE", status.reason);
      if (this.device || this.server || this.connected) {
        throw new WebBluetoothError(
          "BROWSER_BLE_BUSY",
          "Disconnect the current browser Bluetooth session before choosing another H10.",
          true,
        );
      }
      this.onEvent = onEvent;
      this.updateConfig(config);
      this.breathing.reset();
      this.disconnecting = false;
      try {
        this.emit({ kind: "status", message: "Choose your Polar H10 in the browser Bluetooth prompt…" });
        try {
          // Keep requestDevice as the first awaited browser operation so strict Chromium
          // variants still associate the chooser with the initiating click/touch gesture.
          this.device = await navigator.bluetooth.requestDevice({
            filters: [{ namePrefix: "Polar H10" }],
            optionalServices: [UUIDS.pmdService, UUIDS.heartRateService, UUIDS.batteryService],
          });
        } catch (error) {
          throw normalizeChooserError(error);
        }
        this.device.addEventListener("gattserverdisconnected", this.boundDisconnected);
        this.emit({ kind: "status", message: "Connecting and discovering Polar PMD services…" });
        this.server = await this.connectGatt();
        const pmdService = await this.server.getPrimaryService(UUIDS.pmdService);
        this.control = await pmdService.getCharacteristic(UUIDS.pmdControl);
        this.pmdData = await pmdService.getCharacteristic(UUIDS.pmdData);
        this.control.addEventListener("characteristicvaluechanged", this.boundControl);
        this.pmdData.addEventListener("characteristicvaluechanged", this.boundPmd);
        await this.pmdData.startNotifications();
        await this.control.startNotifications();

        try {
          const heartRateService = await this.server.getPrimaryService(UUIDS.heartRateService);
          this.heartRate = await heartRateService.getCharacteristic(UUIDS.heartRateMeasurement);
          this.heartRate.addEventListener("characteristicvaluechanged", this.boundHeartRate);
          await this.heartRate.startNotifications();
        } catch {
          this.heartRate = null;
        }

        await this.writeControl(COMMANDS.startEcg);
        await this.writeControl(COMMANDS.startAccelerometer);
        let batteryPercent = null;
        try {
          const batteryService = await this.server.getPrimaryService(UUIDS.batteryService);
          const battery = await batteryService.getCharacteristic(UUIDS.batteryLevel);
          const value = asBytes(await battery.readValue());
          batteryPercent = value.length ? value[0] : null;
        } catch {
          batteryPercent = null;
        }
        this.connected = true;
        this.wakeLockWanted = true;
        const screenAwake = await this.requestWakeLock();
        this.emit({
          kind: "connection",
          connected: true,
          streaming: true,
          simulated: false,
          transport: "web-bluetooth",
          deviceName: this.device.name || "Polar H10",
          batteryPercent,
          message: screenAwake
            ? "Experimental browser BLE · foreground screen wake lock active"
            : "Experimental browser BLE · keep this tab visible and the screen awake",
        });
      } catch (error) {
        await this.disconnect({ emit: false });
        this.onEvent = null;
        throw normalizeBrowserError(error);
      }
    }

    async connectGatt() {
      let lastError = null;
      for (let attempt = 0; attempt < 2; attempt += 1) {
        try {
          return await this.device.gatt.connect();
        } catch (error) {
          lastError = error;
          const transient = error?.name === "NetworkError" || error?.name === "AbortError";
          if (!transient || attempt === 1) throw error;
          this.emit({ kind: "status", message: "Retrying the browser GATT connection…" });
          await new Promise((resolve) => window.setTimeout(resolve, GATT_CONNECT_RETRY_DELAY_MS));
        }
      }
      throw lastError;
    }

    async writeControl(command) {
      const value = Uint8Array.from(command);
      if (typeof this.control.writeValueWithResponse === "function") {
        await this.control.writeValueWithResponse(value);
      } else {
        await this.control.writeValue(value);
      }
    }

    async requestWakeLock() {
      if (!this.wakeLockWanted || document.visibilityState !== "visible"
        || typeof navigator.wakeLock?.request !== "function") return false;
      if (this.wakeLock && !this.wakeLock.released) return true;
      try {
        const sentinel = await navigator.wakeLock.request("screen");
        this.wakeLock = sentinel;
        sentinel.addEventListener("release", () => {
          if (this.wakeLock === sentinel) this.wakeLock = null;
        }, { once: true });
        return true;
      } catch {
        this.wakeLock = null;
        return false;
      }
    }

    async releaseWakeLock() {
      this.wakeLockWanted = false;
      const sentinel = this.wakeLock;
      this.wakeLock = null;
      if (sentinel && !sentinel.released) {
        try { await sentinel.release(); } catch { /* best effort */ }
      }
    }

    handlePmd(event) {
      try {
        const frame = decodePmd(event.target.value);
        this.emit(frame);
        if (frame.kind === "accelerometer"
          && [
            "breathing_volume", "breathing_signal_confidence", "breathing_signal_ready",
            "breathing_phase", "acc_breathing_magnitude", "breathing_calibration", "breathing_axis_range",
          ].some((id) => this.outputs.has(id))) {
          const snapshot = typeof this.breathing.pushTimed === "function"
            ? this.breathing.pushTimed(frame.samples, frame.sensorTimestampNs)
            : this.breathing.push(frame.samples);
          if (snapshot) {
            this.emit({
              kind: "metrics",
              sensorTimestampNs: frame.sensorTimestampNs,
              ...(snapshot.presentationPoints?.length
                ? { breathingPresentationPoints: snapshot.presentationPoints }
                : {}),
              ...(snapshot.lost ? { breathingGapBefore: true } : {}),
              values: snapshot.values,
            });
          }
        }
      } catch (error) {
        this.emit({ kind: "error", message: `Skipped malformed browser PMD frame: ${error.message}` });
      }
    }

    handleHeartRate(event) {
      const frame = decodeHeartRate(event.target.value);
      const values = [{ id: "heart_rate", value: frame.beatsPerMinute }];
      for (const interval of frame.rrIntervalsMs) values.push({ id: "rr_interval", value: interval });
      this.emit({ kind: "metrics", values });
    }

    handleControl(event) {
      const bytes = asBytes(event.target.value);
      if (bytes.length >= 4 && bytes[0] === 0xf0 && bytes[3] !== 0) {
        this.emit({
          kind: "error",
          message: `Polar PMD rejected command 0x${bytes[1].toString(16)} for measurement 0x${bytes[2].toString(16)} (status ${bytes[3]}).`,
        });
      }
    }

    async disconnect({ emit = true } = {}) {
      if (this.disconnecting) return { emitted: false };
      this.disconnecting = true;
      const wasConnected = this.connected;
      const deviceName = this.device?.name || "Polar H10";
      try {
        if (this.server?.connected && this.control) {
          try { await this.writeControl(COMMANDS.stopEcg); } catch { /* best effort */ }
          try { await this.writeControl(COMMANDS.stopAccelerometer); } catch { /* best effort */ }
        }
        await this.stopCharacteristic(this.pmdData, this.boundPmd);
        await this.stopCharacteristic(this.control, this.boundControl);
        await this.stopCharacteristic(this.heartRate, this.boundHeartRate);
        if (this.device) this.device.removeEventListener("gattserverdisconnected", this.boundDisconnected);
        if (this.server?.connected) this.server.disconnect();
      } finally {
        await this.releaseWakeLock();
        this.connected = false;
        this.server = null;
        this.control = null;
        this.pmdData = null;
        this.heartRate = null;
        this.device = null;
        this.disconnecting = false;
      }
      if (emit && (wasConnected || this.onEvent)) {
        this.emitBreathingUnavailable();
        this.emit({
          kind: "connection",
          connected: false,
          streaming: false,
          simulated: false,
          transport: "web-bluetooth",
          deviceName,
          batteryPercent: null,
          message: "Browser Bluetooth disconnected",
        });
      }
      if (emit) this.onEvent = null;
      return { emitted: emit && Boolean(wasConnected) };
    }

    async stopCharacteristic(characteristic, listener) {
      if (!characteristic) return;
      characteristic.removeEventListener("characteristicvaluechanged", listener);
      try { await characteristic.stopNotifications(); } catch { /* best effort */ }
    }

    handleDisconnected() {
      if (this.disconnecting) return;
      const deviceName = this.device?.name || "Polar H10";
      this.connected = false;
      void this.releaseWakeLock();
      this.server = null;
      this.control = null;
      this.pmdData = null;
      this.heartRate = null;
      this.device = null;
      this.emitBreathingUnavailable();
      this.emit({
        kind: "connection",
        connected: false,
        streaming: false,
        simulated: false,
        transport: "web-bluetooth",
        deviceName,
        batteryPercent: null,
        message: "The H10 left Bluetooth range or disconnected",
      });
      this.onEvent = null;
    }

    emitBreathingUnavailable() {
      const values = [
        { id: "breathing_phase", value: 0 },
        { id: "breathing_signal_confidence", value: 0 },
        { id: "breathing_signal_ready", value: 0 },
      ].filter(({ id }) => this.outputs.has(id));
      if (values.length) this.emit({ kind: "metrics", values });
    }

    emit(event) {
      if (typeof this.onEvent === "function") this.onEvent(event);
    }
  }

  const session = new PolarWebBluetoothSession();
  function createConfiguredBreathingProcessor(settings = {}) {
    const normalized = clampBreathingSettings(settings);
    return normalized.volumeMode === "legacy-v0" && normalized.stateMode === "legacy-v0"
      ? new BreathingProcessor(normalized)
      : new TimedBreathingProcessor(normalized);
  }
  const api = {
    UUIDS,
    COMMANDS,
    moduleId: "web-bluetooth-polar-h10",
    supportStatus,
    decodeHeartRate,
    decodePmd,
    createBreathingProcessor(settings) {
      return new BreathingProcessor(settings);
    },
    createTimedBreathingProcessor(settings) {
      return new TimedBreathingProcessor(settings);
    },
    createConfiguredBreathingProcessor,
    createBreathingPresentation(mode, options) {
      return new BreathingPresentation(mode, options);
    },
    connect(onEvent, config) {
      return session.connect(onEvent, config);
    },
    disconnect() {
      return session.disconnect();
    },
    updateConfig(config) {
      session.updateConfig(config);
    },
  };

  window.PolarWebBluetooth = Object.freeze(api);
})();
