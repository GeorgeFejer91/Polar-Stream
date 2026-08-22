(() => {
  "use strict";

  const runtime = window.PolarRuntimeApi;
  const isNative = runtime.isNative;
  const browserSession = window.PolarBrowserSession;
  const audioDataLink = window.PolarAudioDataLink;
  const preferences = window.PolarPreferences;
  const previewFixtureApi = window.PolarPreviewFixture;
  const formulaPreview = window.PolarFormulaPreview;
  let metricPreviews = window.PolarMetricPreviews || null;
  let metricPreviewsPromise = null;
  let previewRecording = null;
  let previewRecordingPromise = null;
  const isInterfaceRenderer = new URLSearchParams(window.location.search).has("renderer");
  const svgNamespace = "http://www.w3.org/2000/svg";
  let metricPreviewSequence = 0;
  let browserRecorderCapacityNotified = false;
  let browserLifecycleWarningPending = false;
  let formulaPreviewAnimationId = 0;
  let formulaPreviewAnimationResult = null;
  let formulaPreviewAnimationStartedAt = 0;
  let formulaPreviewLastDrawAt = 0;

  const evidenceLinks = {
    hrv: ["Shaffer & Ginsberg (2017)", "https://www.frontiersin.org/journals/public-health/articles/10.3389/fpubh.2017.00258/full"],
    breathing: ["Schipper et al. (2021)", "https://pubmed.ncbi.nlm.nih.gov/33739305/"],
    complexity: ["Bará et al. (2024)", "https://doi.org/10.1016/j.bbe.2024.04.004"],
    resonance: ["Sévoz-Couche & Laborde (2022)", "https://www.sciencedirect.com/science/article/abs/pii/S0149763422000653"],
    quality: ["Smital et al. (2020)", "https://pubmed.ncbi.nlm.nih.gov/31995473/"],
    stress: ["Immanuel et al. (2023)", "https://pmc.ncbi.nlm.nih.gov/articles/PMC10614455/"],
    exciteometer: ["Excite-O-Meter source implementation", "https://github.com/luisqtr/exciteometer/blob/main/docs/1_UserManual.md#scientific-disclaimer"],
  };
  const fallbackSourceGroups = {
    hrv: [
      ["ESC/NASPE Task Force (1996)", "https://pubmed.ncbi.nlm.nih.gov/8737210/"],
      ["Laborde, Mosley & Thayer (2017)", "https://pubmed.ncbi.nlm.nih.gov/28265249/"],
      evidenceLinks.hrv,
    ],
    breathing: [
      evidenceLinks.breathing,
      ["Bates et al. (2021)", "https://pubmed.ncbi.nlm.nih.gov/33937389/"],
      ["Aliverti (2024)", "https://pubmed.ncbi.nlm.nih.gov/38392009/"],
    ],
    complexity: [
      evidenceLinks.complexity,
      ["Angelini et al. (2007)", "https://pubmed.ncbi.nlm.nih.gov/17950584/"],
      evidenceLinks.breathing,
    ],
    resonance: [
      evidenceLinks.resonance,
      ["Lehrer & Gevirtz (2014)", "https://pubmed.ncbi.nlm.nih.gov/25101026/"],
      ["Laborde, Mosley & Thayer (2017)", "https://pubmed.ncbi.nlm.nih.gov/28265249/"],
    ],
    quality: [
      evidenceLinks.quality,
      ["Satija, Ramkumar & Manikandan (2018)", "https://pubmed.ncbi.nlm.nih.gov/29994590/"],
      ["Liu et al. (2022)", "https://pubmed.ncbi.nlm.nih.gov/36359421/"],
    ],
    stress: [
      evidenceLinks.stress,
      ["Quintero et al. (2021)", "https://michagaebler.github.io/doc/Quintero2021_EoM.pdf"],
      ["Laborde, Mosley & Thayer (2017)", "https://pubmed.ncbi.nlm.nih.gov/28265249/"],
    ],
    exciteometer: [
      evidenceLinks.exciteometer,
      ["Quintero et al. (2021)", "https://michagaebler.github.io/doc/Quintero2021_EoM.pdf"],
      evidenceLinks.stress,
    ],
  };
  function fallbackSources(id, source) {
    const candidates = id === "raw_force"
      ? [
        ["Vernier GDX-RB user manual", "https://www.vernier.com/manuals/GDX-RB"],
        ["Vernier Go Direct examples", "https://github.com/VernierST/godirect-examples"],
        evidenceLinks.breathing,
      ]
      : fallbackSourceGroups[source] || fallbackSourceGroups.hrv;
    return candidates.slice(0, 3).map(([label, url]) => ({ label, url }));
  }
  const fallbackMetric = (id, streamSuffix, label, unit, category, source = "hrv", detail = "Real-time derived metric", raw = false, normalizable = true, rateHz = 0) => ({
    id, streamSuffix, label, detail, unit, category, raw, normalizable, rateHz,
    evidence: raw ? "device signal" : "research metric",
    citationLabel: evidenceLinks[source][0], citationUrl: evidenceLinks[source][1],
    sources: fallbackSources(id, source),
    keywords: `${label} ${category}`.toLowerCase(),
    explainer: `${label} is exposed as a descriptive research signal using the formula named in this card. Its physiological meaning depends on signal quality, recording context and the cited method; it is not a diagnosis or a standalone emotional-state measure.`,
  });
  const legacyFallbackCatalog = [
    fallbackMetric("raw_ecg", "rawECG", "Raw ECG", "µV", "Raw signals", "quality", "Unfiltered H10 voltage · 130 Hz", true, false, 130),
    fallbackMetric("raw_acc", "rawACC", "Raw accelerometer", "mg", "Raw signals", "breathing", "X, Y and Z · 200 Hz", true, false, 200),
    fallbackMetric("acc_magnitude", "accMagnitude", "3D acceleration magnitude", "g", "Raw signals", "breathing", "√(x²+y²+z²) · device motion", false, true, 200),
    ...[["ecg_mean","ecgMean","ECG window mean"],["ecg_rms","ecgRms","ECG RMS amplitude"],["ecg_peak_to_peak","ecgPeakToPeak","ECG peak-to-peak"],["ecg_sd","ecgSd","ECG standard deviation"]].map(([id,suffix,label]) => fallbackMetric(id,suffix,label,"µV","ECG features","quality","Five-second rolling signal feature",false,true,2)),
    fallbackMetric("heart_rate", "heartRate", "Heart rate", "bpm", "Heart rate", "hrv", "H10 device-derived", false, true),
    fallbackMetric("rr_interval", "rrInterval", "RR interval", "ms", "Heart rate", "hrv", "Accepted beat-to-beat interval"),
    fallbackMetric("mean_nn", "meanNN", "Mean NN interval", "ms", "Heart rate"),
    fallbackMetric("mean_heart_rate", "meanHeartRate", "Mean heart rate", "bpm", "Heart rate"),
    ...[["rmssd","rmssd","RMSSD","ms"],["ln_rmssd","lnRMSSD","lnRMSSD","ln(ms)"],["sdnn","sdnn","SDNN","ms"],["pnn50","pNN50","pNN50","%"],["sd1","sd1","Poincaré SD1","ms"]].map(([id,suffix,label,unit]) => fallbackMetric(id,suffix,label,unit,"HRV & relaxation")),
    ...[["coherence","coherence","Normalized coherence","0–1"],["coherence_confidence","coherenceConfidence","Coherence confidence","0–1"],["heartmath_coherence","heartMathCoherence","HeartMath-style coherence ratio","ratio"],["coherence_peak_frequency","coherencePeakFrequency","Coherence peak frequency","Hz"],["coherence_peak_power","coherencePeakPower","Coherence peak-band power","ms²"],["coherence_total_power","coherenceTotalPower","Coherence total power","ms²"]].map(([id,suffix,label,unit]) => fallbackMetric(id,suffix,label,unit,"Coherence","resonance")),
    fallbackMetric("acc_breathing_magnitude", "accBreathingMagnitude", "ACC breathing magnitude estimate", "g", "Breathing", "breathing", "Smoothed selected-axis chest-motion projection", false, true, 20),
    fallbackMetric("breathing_volume", "breathingVolume", "ACC breathing waveform", "0–1", "Breathing", "breathing", "Robustly normalized relative chest-motion projection; not lung volume", false, true, 20),
    fallbackMetric("breathing_signal_confidence", "breathingSignalConfidence", "ACC breathing signal confidence", "0–1", "Breathing", "breathing", "Range, motion, coverage, and periodicity quality index", false, false, 20),
    fallbackMetric("breathing_signal_ready", "breathingSignalReady", "ACC breathing signal ready", "0/1", "Breathing", "breathing", "Calibration, freshness, and motion gate", false, false, 20),
    fallbackMetric("breathing_phase", "breathingPhase", "Breath phase classifier", "class", "Breathing", "breathing", "+1 inhale · −1 exhale · 0 pause or not ready", false, false, 20),
    fallbackMetric("breathing_calibration", "breathingCalibration", "Breathing calibration", "0–1", "Breathing", "breathing", "Principal-axis calibration progress", false, false, 4),
    fallbackMetric("breathing_axis_range", "breathingAxisRange", "Breathing axis range", "g", "Breathing", "breathing"),
    fallbackMetric("breathing_rate", "breathingRate", "Breathing rate", "breaths/min", "Breathing", "breathing"),
    fallbackMetric("breathing_dynamics_confidence", "breathingDynamicsConfidence", "Breathing-dynamics confidence", "0–1", "Breathing dynamics", "complexity"),
    ...["mean","sd","cv","acw50","psd_slope","lzc","sampen","mse"].flatMap((feature) => ["interval","amplitude"].map((kind) => {
      const names = { mean:"mean", sd:"SD", cv:"CV", acw50:"ACW50", psd_slope:"PSD slope", lzc:"Lempel–Ziv", sampen:"sample entropy", mse:"multiscale entropy" };
      const units = kind === "interval" ? {mean:"s",sd:"s",cv:"ratio",acw50:"breaths",psd_slope:"slope",lzc:"0–1",sampen:"entropy",mse:"entropy AUC"} : {mean:"0–1",sd:"0–1",cv:"ratio",acw50:"breaths",psd_slope:"slope",lzc:"0–1",sampen:"entropy",mse:"entropy AUC"};
      const camel = feature.split("_").map((part,index) => index ? part[0].toUpperCase()+part.slice(1) : part).join("");
      return fallbackMetric(`breath_${kind}_${feature}`, `breath${kind[0].toUpperCase()+kind.slice(1)}${camel[0].toUpperCase()+camel.slice(1)}`, `Breath ${kind} ${names[feature]}`, units[feature], "Breathing dynamics", "complexity");
    })),
    fallbackMetric("excitement_score", "excitementScore", "Excite-O-Meter excitement score", "0–1", "Excitation (experimental)", "exciteometer", "1 − mean[Φ(zRR), Φ(zRMSSD)] · live provisional", false, false),
    fallbackMetric("excitometer", "excitometer", "Activation composite (experimental)", "0–1", "Excitation (experimental)", "stress", "Within-session HR ↑ plus lnRMSSD ↓"),
  ];
  const staticCatalog = window.PolarMetricCatalog || legacyFallbackCatalog;
  const rawForceMetric = fallbackMetric(
    "raw_force", "rawForce", "Raw Go Direct force", "N", "Raw signals", "breathing",
    "Verified GDX-RB Force (N) · 10 Hz preferred, metadata fallback", true, false, 0,
  );
  const fallbackCatalog = staticCatalog.some((metric) => metric.id === "raw_force")
    ? staticCatalog
    : [...staticCatalog.slice(0, 2), rawForceMetric, ...staticCatalog.slice(2)];

  const visualDefinitions = {
    raw_ecg: { label: "Raw ECG", unit: "µV", rate: 130, color: "#d85151", symmetric: true },
    raw_acc: {
      label: "Raw accelerometer · X/Y/Z", unit: "mg", rate: 200,
      channels: [
        { buffer: "acc_x", label: "X", color: "#3b78aa", symmetric: true },
        { buffer: "acc_y", label: "Y", color: "#168259", symmetric: true },
        { buffer: "acc_z", label: "Z", color: "#a66d19", symmetric: true },
      ],
    },
    raw_force: { label: "Raw Go Direct force", unit: "N", rate: 10, color: "#00c2ff" },
    heart_rate: { label: "Heart rate", unit: "bpm", rate: 1, color: "#d85151" },
    rr_interval: { label: "RR interval", unit: "ms", rate: 2, color: "#6c62a8" },
    acc_magnitude: { label: "3D acceleration magnitude", unit: "g", rate: 200, color: "#3b78aa" },
    acc_breathing_magnitude: { label: "ACC breathing magnitude estimate", unit: "g", rate: 20, color: "#3b78aa" },
    rmssd: { label: "RMSSD", unit: "ms", rate: 1, color: "#168259" },
  };

  const accLibraryIds = new Set(fallbackCatalog
    .filter((metric) => metric.id === "raw_acc"
      || metric.id === "raw_force"
      || metric.id === "acc_magnitude"
      || metric.category === "Breathing"
      || metric.category === "Breathing dynamics")
    .map((metric) => metric.id));
  const breathingOutputIds = new Set([
    "acc_breathing_magnitude",
    "breathing_volume",
    "breathing_phase",
    "breathing_calibration",
    "breathing_axis_range",
    "breathing_signal_confidence",
    "breathing_signal_ready",
  ]);
  const formulaSources = Object.freeze({
    ecg: { label: "ECG · 130 Hz", variables: "ecg", color: "#d85151" },
    accelerometer: { label: "Accelerometer · 200 Hz", variables: "x, y, z", color: "#3b78aa" },
    heartRate: { label: "Heart rate · event rate", variables: "hr", color: "#a65757" },
    rrInterval: { label: "RR interval · beat rate", variables: "rr", color: "#6c62a8" },
  });

  function defaultBreathingSettings() {
    return {
      axes: [true, false, true],
      calibrationWindowSeconds: 12,
      minimumAxisRangeG: 0.01,
      smoothingWindowSeconds: 0.75,
      sensitivity: 0.60,
      staleTimeoutSeconds: 3,
      invertDirection: false,
      adaptiveBounds: true,
      adaptiveWindowSeconds: 20,
      lowerQuantile: 0.05,
      upperQuantile: 0.95,
    };
  }

  class RingBuffer {
    constructor(capacity = 4096) {
      this.values = new Float32Array(capacity);
      this.capacity = capacity;
      this.length = 0;
      this.cursor = 0;
    }

    push(value) {
      if (!Number.isFinite(value)) return;
      this.values[this.cursor] = value;
      this.cursor = (this.cursor + 1) % this.capacity;
      this.length = Math.min(this.length + 1, this.capacity);
    }

    pushMany(values) {
      for (const value of values) this.push(Number(value));
    }

    tailSize(count) {
      return Math.min(this.length, count);
    }

    tailValue(index, size) {
      const start = (this.cursor - size + this.capacity) % this.capacity;
      return this.values[(start + index) % this.capacity];
    }

    latest() {
      return this.length ? this.values[(this.cursor - 1 + this.capacity) % this.capacity] : null;
    }

    clear() {
      this.length = 0;
      this.cursor = 0;
    }
  }

  const bufferIds = new Set(Object.entries(visualDefinitions).flatMap(([id, definition]) => (
    [id, ...(definition.channels?.map((channel) => channel.buffer) || [])]
  )));
  function createBufferBank() {
    return Object.fromEntries([...bufferIds].map((id) => [id, new RingBuffer()]));
  }
  const sourceBuffers = new Map();
  let buffers = createBufferBank();
  const elements = {};
  const ids = [
    "app-state-dot", "app-state-text", "platform-label", "runtime-path-label", "input-state", "connection-card",
    "device-name", "connection-detail", "disconnect-button", "connection-meta", "battery-value", "connection-metric-1-label", "connection-metric-1-value", "connection-metric-2-label", "connection-metric-2-value", "active-source-strip",
    "scan-button", "scan-caption", "device-list", "activity-list", "output-state", "raw-ecg-value",
    "raw-acc-x", "raw-acc-y", "raw-acc-z", "raw-force-value", "ecg-spark", "stream-name", "stream-name-label", "lsl-toggle", "osc-toggle", "csv-toggle", "audio-toggle",
    "lsl-detail", "osc-detail", "csv-detail", "audio-detail", "lsl-destination-row", "osc-destination-row", "native-output-browser-error", "native-output-browser-error-text", "desktop-app-download", "browser-local-destination", "browser-recorder-actions", "included-count", "output-chips", "open-output-dialog", "visual-device", "visual-source",
    "visual-current", "visual-unit", "render-rate", "chart-shell", "signal-canvas", "visual-legend",
    "chart-empty", "y-max", "y-min", "footer-status", "sample-counter", "output-dialog",
    "metric-options", "metric-detail", "metric-back-button", "dialog-output-status", "save-metric-output", "toast-region",
    "stream-name-preview", "metric-search", "metric-filters", "metric-library-summary",
    "metric-family-toggle", "metric-family-context", "metric-family-note",
    "adjust-visual", "visual-window-label", "visual-scale-label", "module-dialog", "module-dialog-title",
    "module-dialog-intro", "module-settings", "module-dialog-status", "save-module-settings",
    "pipeline-title", "pipeline-detail", "browser-export-button",
    "browser-discard-button", "browser-recorder-count", "browser-recorder-status",
    "open-formula-lab", "formula-dialog", "formula-dialog-title", "new-custom-formula",
    "formula-saved-list", "formula-template-buttons", "formula-name", "formula-source",
    "formula-unit", "formula-expression", "formula-keyboard", "formula-preview-current",
    "formula-preview-canvas", "formula-preview-note", "formula-validation-status",
    "delete-custom-formula", "save-custom-formula",
  ];
  for (const id of ids) elements[id] = document.getElementById(id);
  const signalContext = elements["signal-canvas"].getContext("2d", {
    alpha: true,
    desynchronized: true,
  });

  const app = {
    connected: false,
    connecting: false,
    scanning: false,
    configuring: false,
    catalog: fallbackCatalog,
    outputs: new Set(["raw_ecg", "raw_acc", "raw_force"]),
    metricOptions: {},
    customFormulas: [],
    formulaDraft: null,
    editingFormulaId: null,
    formulaPreviewTimer: null,
    formulaNoticesShown: new Set(),
    visualNormalizers: {},
    selectedMetricId: null,
    editingModuleId: null,
    moduleDraft: null,
    libraryMetricDraft: null,
    metricFamily: "ecg",
    metricFilter: "All",
    metricSearch: "",
    streamName: "Polar-H10",
    selectedVisual: "raw_ecg",
    sampleCount: 0,
    outputSequence: 0,
    connectionGeneration: 0,
    currentDeviceId: null,
    currentInputKind: null,
    activeSources: new Map(),
    selectedSourceId: null,
    pendingDevice: null,
    devices: [],
    breathingSettings: defaultBreathingSettings(),
    phaseMotion: { level: 0.5, velocity: 0, lastAt: 0 },
    preferences: isNative
      ? { streamName: null, lastDevice: null, outputConfig: null }
      : preferences.load(),
    activity: [{ time: "NOW", message: isNative ? "Bluetooth interface ready" : "Browser demo ready" }],
  };

  function normalizeStreamBase(value) {
    if (typeof value !== "string") return null;
    let normalized = "";
    let separatorPending = false;
    for (const character of String(value).trim()) {
      if (/[A-Za-z0-9]/.test(character)) {
        if (separatorPending && normalized && !normalized.endsWith("_") && !normalized.endsWith("-")) {
          normalized += "_";
        }
        normalized += character;
        separatorPending = false;
      } else if (character === "-") {
        normalized += character;
        separatorPending = false;
      } else {
        separatorPending = true;
      }
    }
    normalized = normalized.replace(/^[_-]+|[_-]+$/g, "");
    return normalized && normalized.length <= 64 ? normalized : null;
  }

  function streamOutputName(metric, value = app.streamName) {
    const base = normalizeStreamBase(value);
    const suffix = metric.streamSuffix
      || fallbackCatalog.find((candidate) => candidate.id === metric.id)?.streamSuffix
      || metric.id;
    const slot = app?.activeSources?.get(app.selectedSourceId)?.slot;
    return base ? `${base}${slot ? `_${slot}` : ""}_${suffix}` : `—_${suffix}`;
  }

  function newFormulaId() {
    if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
    const bytes = new Uint8Array(16);
    globalThis.crypto?.getRandomValues?.(bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = [...bytes].map((value) => value.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }

  function normalizeFormulaDraft(value = {}) {
    const source = formulaSources[value.source] ? value.source : "ecg";
    return {
      id: value.id || newFormulaId(),
      name: String(value.name || "Processed_ECG"),
      source,
      expression: String(value.expression || formulaPreview.sourceMap[source].variables[0]),
      unit: String(value.unit || (source === "ecg" ? "µV" : source === "accelerometer" ? "mg" : source === "heartRate" ? "bpm" : "ms")),
      enabled: value.enabled !== false,
    };
  }

  function customStreamName(formula, value = app.streamName) {
    const base = normalizeStreamBase(value);
    const suffix = String(formula?.name || "custom").trim().replace(/[^A-Za-z0-9_-]+/g, "_").replace(/^[_-]+|[_-]+$/g, "") || "custom";
    return base ? `${base}_${suffix}` : `—_${suffix}`;
  }

  function setTopStatus(message, state = "idle") {
    elements["app-state-text"].textContent = message;
    elements["app-state-dot"].className = `state-dot${state === "idle" ? "" : ` ${state}`}`;
    elements["footer-status"].textContent = message;
  }

  function addActivity(message) {
    const time = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    app.activity.unshift({ time, message });
    app.activity.length = Math.min(app.activity.length, 3);
    elements["activity-list"].replaceChildren(...app.activity.map((item) => {
      const row = document.createElement("li");
      const stamp = document.createElement("time");
      const text = document.createElement("span");
      stamp.textContent = item.time;
      text.textContent = item.message;
      row.append(stamp, text);
      return row;
    }));
  }

  function toast(message, error = false) {
    const region = elements["toast-region"];
    const key = `${error ? "error" : "status"}:${message}`;
    if ([...region.children].some((child) => child.dataset.toastKey === key)) return;
    while (region.childElementCount >= 3) region.firstElementChild?.remove();
    const node = document.createElement("div");
    node.className = `toast${error ? " error" : ""}`;
    node.dataset.toastKey = key;
    node.textContent = message;
    region.append(node);
    window.setTimeout(() => node.remove(), 4200);
  }

  function ensureMetricPreviews() {
    if (metricPreviews) return Promise.resolve(metricPreviews);
    if (metricPreviewsPromise) return metricPreviewsPromise;
    metricPreviewsPromise = new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = "metric-previews.js";
      script.async = true;
      script.addEventListener("load", () => {
        metricPreviews = window.PolarMetricPreviews || null;
        if (metricPreviews) resolve(metricPreviews);
        else reject(new Error("Metric preview data did not initialize."));
      }, { once: true });
      script.addEventListener("error", () => reject(new Error("Metric previews could not be loaded.")), { once: true });
      document.head.append(script);
    }).catch((error) => {
      metricPreviewsPromise = null;
      throw error;
    });
    return metricPreviewsPromise;
  }

  function ensurePreviewRecording() {
    if (previewRecording) return Promise.resolve(previewRecording);
    if (previewRecordingPromise) return previewRecordingPromise;
    previewRecordingPromise = fetch("data/preview-recording.json", { cache: "no-cache" })
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        return response.json();
      })
      .then((value) => {
        previewRecording = previewFixtureApi.validateFixture(value);
        return previewRecording;
      })
      .catch((error) => {
        previewRecordingPromise = null;
        throw error;
      });
    return previewRecordingPromise;
  }

  function renderRuntimeContext({ simulated = false, transport = null } = {}) {
    if (simulated) {
      elements["runtime-path-label"].textContent = "Seamless recorded H10 loop";
      elements["pipeline-title"].textContent = "Recorded preview loops locally";
      elements["pipeline-detail"].textContent = "The canonical anonymized ECG and ACC recording repeats as one continuous preview signal; no BLE, LSL, or OSC connection is opened.";
      return;
    }
    if (isInterfaceRenderer) {
      elements["runtime-path-label"].textContent = "Deterministic interface test";
      return;
    }
    if (transport === "web-bluetooth") {
      elements["runtime-path-label"].textContent = "Browser Bluetooth · experimental";
      elements["pipeline-title"].textContent = "Acquisition stays in this tab";
      elements["pipeline-detail"].textContent = "Chromium reads H10 ECG, ACC, HR and RR directly. No companion or installed process is used.";
      return;
    }
    if (runtime.isBrowser) {
      elements["runtime-path-label"].textContent = "Browser-local inputs";
      elements["pipeline-title"].textContent = "Choose live or recorded input";
      elements["pipeline-detail"].textContent = "Connect an H10 with Chromium Web Bluetooth, or replay the anonymized 60-second H10 recording. All processing stays in this tab.";
      return;
    }
    elements["runtime-path-label"].textContent = "Native data path";
    elements["pipeline-title"].textContent = "Acquisition stays native";
    elements["pipeline-detail"].textContent = "The chart receives display-rate batches; LSL and OSC do not wait for rendering.";
  }

  async function initialize() {
    let bootstrap = {
      config: { streamName: "Polar-H10", lslEnabled: false, oscEnabled: false, csvEnabled: false, audioEnabled: false, outputs: ["raw_ecg", "raw_acc"], metricOptions: {}, customFormulas: [] },
      platform: "browser preview",
      metricCatalog: fallbackCatalog,
    };
    try {
      bootstrap = await runtime.getBootstrap(bootstrap);
    } catch (error) {
      toast(runtime.formatError(error), true);
    }

    app.catalog = bootstrap.metricCatalog || fallbackCatalog;
    if (isNative && !bootstrap.hasSavedPreferences) {
      const legacy = preferences.load();
      const outputConfig = legacy.outputConfig || (legacy.streamName
        ? { ...structuredClone(bootstrap.config), streamName: legacy.streamName }
        : null);
      if (outputConfig || legacy.lastDevice) {
        try {
          bootstrap.preferences = await runtime.migrateLegacyPreferences({
            outputConfig,
            lastDevice: legacy.lastDevice,
          });
          bootstrap.config = bootstrap.preferences.outputConfig;
        } catch (error) {
          toast(`Previous preferences could not be imported. ${runtime.formatError(error)}`, true);
        }
      }
    }
    if (isNative && bootstrap.preferences) {
      app.preferences = {
        streamName: bootstrap.preferences.outputConfig?.streamName || null,
        outputConfig: bootstrap.preferences.outputConfig || null,
        lastDevice: bootstrap.preferences.lastDevice || null,
      };
    }
    const initialConfig = isNative
      ? bootstrap.preferences?.outputConfig || bootstrap.config || {}
      : app.preferences.outputConfig || bootstrap.config || {};
    app.outputs = new Set(initialConfig.outputs || ["raw_ecg", "raw_acc", "raw_force"]);
    app.metricOptions = structuredClone(initialConfig.metricOptions || {});
    app.customFormulas = (initialConfig.customFormulas || []).map(normalizeFormulaDraft);
    app.breathingSettings = configuredBreathingSettings(app.metricOptions);
    installCatalogVisuals();
    installCustomFormulaVisuals();
    app.streamName = normalizeStreamBase(app.preferences.streamName)
      || normalizeStreamBase(initialConfig.streamName)
      || "Polar-H10";
    elements["stream-name"].value = app.streamName;
    elements["lsl-toggle"].checked = Boolean(initialConfig.lslEnabled);
    elements["osc-toggle"].checked = Boolean(initialConfig.oscEnabled);
    elements["csv-toggle"].checked = runtime.isBrowser ? false : Boolean(initialConfig.csvEnabled);
    // Web Audio must be resumed from an explicit user gesture. Never restore
    // an emitting audio modem silently after launch or reload.
    elements["audio-toggle"].checked = false;
    elements["desktop-app-download"].href = "https://github.com/GeorgeFejer91/Polar-Stream/releases/latest";
    if (runtime.isBrowser) {
      elements["lsl-toggle"].checked = false;
      elements["osc-toggle"].checked = false;
      elements["browser-local-destination"].hidden = false;
      elements["browser-recorder-actions"].hidden = false;
      elements["stream-name-label"].textContent = "Signal base name";
    }
    document.body.dataset.runtime = runtime.mode;
    elements["platform-label"].textContent = isInterfaceRenderer ? "RENDERER" : String(bootstrap.platform || "local").toUpperCase();
    renderRuntimeContext();
    if (runtime.isDemo) {
      elements["scan-caption"].textContent = "Web Bluetooth or recorded preview";
      elements["scan-button"].querySelector("span").textContent = "Choose Polar H10";
    } else if (isInterfaceRenderer) {
      elements["scan-caption"].textContent = "Deterministic background render";
    }

    app.devices = runtime.getInputModules();
    renderDevices(app.devices);
    if (app.devices.length) {
      elements["input-state"].textContent = runtime.isBrowser ? "Browser ready" : "Mock ready";
      elements["connection-detail"].textContent = "Choose browser Bluetooth for a live H10, or replay the anonymized recording.";
    }

    renderMetricFilters();
    renderOutputs();
    installInteractions();
    await configureOutputs({ quiet: true });
    if (runtime.isBrowser && browserSession) browserSession.subscribe(renderBrowserRecorder);
    if (audioDataLink) audioDataLink.subscribe(renderAudioOutput);
    resizeCanvas();
    if (!isInterfaceRenderer) requestRender();
    if (app.preferences.lastDevice
      && !runtime.isMockDevice(app.preferences.lastDevice.id)
      && !runtime.isBrowserBluetoothDevice(app.preferences.lastDevice.id)) {
      void scanDevices({ automatic: true });
    }
  }

  function installInteractions() {
    elements["scan-button"].addEventListener("click", () => {
      if (runtime.isBrowser) {
        const browserBluetooth = app.devices.find((device) => runtime.isBrowserBluetoothDevice(device.id));
        if (browserBluetooth?.available !== false) {
          void connectDevice(browserBluetooth);
          return;
        }
      }
      void scanDevices();
    });
    elements["disconnect-button"].addEventListener("click", disconnectDevice);
    elements["lsl-toggle"].addEventListener("change", () => handleNativeDestinationToggle("LSL"));
    elements["osc-toggle"].addEventListener("change", () => handleNativeDestinationToggle("OSC"));
    elements["csv-toggle"].addEventListener("change", async () => {
      if (runtime.isBrowser && browserSession) {
        try {
          if (elements["csv-toggle"].checked) {
            browserSession.start({
              deviceName: elements["device-name"].textContent,
              inputKind: app.currentInputKind || "browser",
            });
            addActivity("Browser CSV recording started");
            toast("Recording all incoming browser data in this tab");
          } else if (browserSession.status().state === "recording") {
            browserSession.stop("user");
            const filename = browserSession.download();
            browserSession.discard();
            addActivity(`Downloaded ${filename}`);
            toast(`Saved ${filename}`);
          }
        } catch (error) {
          elements["csv-toggle"].checked = false;
          toast(error.message || String(error), true);
        }
      }
      await configureOutputs();
    });
    elements["audio-toggle"].addEventListener("change", async () => {
      if (!audioDataLink) return;
      try {
        if (elements["audio-toggle"].checked) {
          await audioDataLink.enable({ streamName: app.streamName });
          addActivity("Experimental audio data output started");
          toast("Stereo PCM data modem active · use a cable or digital recorder");
        } else {
          audioDataLink.disable();
          addActivity("Audio data output stopped");
        }
        await configureOutputs();
      } catch (error) {
        elements["audio-toggle"].checked = false;
        audioDataLink.disable();
        toast(error.message || String(error), true);
        await configureOutputs({ quiet: true });
      }
    });
    if (runtime.isBrowser && browserSession) {
      elements["browser-export-button"].addEventListener("click", () => {
        try {
          const filename = browserSession.download();
          browserSession.discard();
          addActivity(`Downloaded ${filename}`);
          toast(`Saved ${filename}`);
        } catch (error) {
          toast(error.message || String(error), true);
        }
      });
      elements["browser-discard-button"].addEventListener("click", () => {
        if (!window.confirm("Discard the browser recording? It cannot be recovered after this tab closes.")) return;
        browserSession.discard();
        elements["csv-toggle"].checked = false;
        addActivity("Browser recording discarded");
      });
      window.addEventListener("beforeunload", (event) => {
        if (!browserSession.status().hasData) return;
        event.preventDefault();
        event.returnValue = "";
      });
    }
    window.addEventListener("polar-stream-audio-error", (event) => {
      elements["audio-toggle"].checked = false;
      toast(event.detail || "Audio data output stopped.", true);
      void configureOutputs({ quiet: true });
    });

    let nameTimer;
    elements["stream-name"].addEventListener("input", () => {
      app.streamName = elements["stream-name"].value;
      renderOutputs();
      window.clearTimeout(nameTimer);
      nameTimer = window.setTimeout(configureOutputs, 320);
    });

    elements["open-output-dialog"].addEventListener("click", async () => {
      app.selectedMetricId = null;
      app.libraryMetricDraft = null;
      setMetricLibraryView("browse");
      updateMetricFamilyUi();
      renderMetricDetail();
      elements["output-dialog"].showModal();
      const loading = document.createElement("p");
      loading.className = "metric-library-empty";
      loading.textContent = "Loading recorded H10 previews…";
      elements["metric-options"].replaceChildren(loading);
      try {
        await Promise.all([ensureMetricPreviews(), ensurePreviewRecording()]);
      } catch (error) {
        toast(String(error), true);
      }
      if (elements["output-dialog"].open) renderMetricOptions();
    });
    elements["open-formula-lab"].addEventListener("click", () => {
      elements["output-dialog"].close();
      void openFormulaLab();
    });
    elements["new-custom-formula"].addEventListener("click", () => editFormulaDraft());
    for (const id of ["formula-name", "formula-unit", "formula-expression"]) {
      elements[id].addEventListener("input", scheduleFormulaPreview);
    }
    elements["formula-source"].addEventListener("change", () => {
      if (!app.formulaDraft) return;
      app.formulaDraft.source = elements["formula-source"].value;
      renderFormulaKeyboard();
      scheduleFormulaPreview();
    });
    elements["save-custom-formula"].addEventListener("click", () => void saveCustomFormula());
    elements["delete-custom-formula"].addEventListener("click", deleteCustomFormula);
    elements["formula-dialog"].addEventListener("close", () => stopFormulaPreviewAnimation());
    elements["metric-back-button"].addEventListener("click", () => {
      setMetricLibraryView("browse");
      window.requestAnimationFrame(() => {
        elements["metric-options"].querySelector(".metric-option.selected")?.focus({ preventScroll: true });
      });
    });
    elements["save-metric-output"].addEventListener("click", () => {
      const metric = app.catalog.find((candidate) => candidate.id === app.selectedMetricId);
      if (!metric || app.outputs.has(metric.id)) return;
      const support = runtime.outputSupport(metric.id, app.currentInputKind);
      if (!support.supported) {
        toast(support.reason, true);
        return;
      }
      const draft = structuredClone(app.libraryMetricDraft || metricOptionFor(metric.id, { forSelection: true }));
      if (breathingOutputIds.has(metric.id)) {
        if (breathingDraftIsInvalid(draft.processing.breathing)) return;
        app.breathingSettings = structuredClone(draft.processing.breathing);
      }
      app.metricOptions[metric.id] = draft;
      app.outputs.add(metric.id);
      renderOutputs();
      configureOutputs();
      elements["output-dialog"].close();
      toast(`${metric.label} added as ${streamOutputName(metric)}`);
    });
    elements["metric-search"].addEventListener("input", () => {
      app.metricSearch = elements["metric-search"].value.trim().toLowerCase();
      renderMetricOptions();
    });
    elements["metric-filters"].addEventListener("click", (event) => {
      const button = event.target.closest("button[data-category]");
      if (!button) return;
      app.metricFilter = button.dataset.category;
      renderMetricFilters();
      renderMetricOptions();
    });
    elements["metric-family-toggle"].addEventListener("click", (event) => {
      const button = event.target.closest("button[data-family]");
      if (!button || button.dataset.family === app.metricFamily) return;
      app.metricFamily = button.dataset.family;
      app.metricFilter = "All";
      app.metricSearch = "";
      app.selectedMetricId = null;
      app.libraryMetricDraft = null;
      elements["metric-search"].value = "";
      updateMetricFamilyUi();
      renderMetricFilters();
      renderMetricOptions();
    });
    elements["visual-source"].addEventListener("change", () => {
      app.selectedVisual = elements["visual-source"].value;
      updateVisualLabels();
      requestRender();
    });
    elements["visual-device"].addEventListener("change", () => {
      selectSource(elements["visual-device"].value);
    });
    elements["adjust-visual"].addEventListener("click", () => openModuleSettings(optionIdForVisual(app.selectedVisual)));
    elements["save-module-settings"].addEventListener("click", saveModuleSettings);

    elements["output-dialog"].addEventListener("close", () => {
      app.selectedMetricId = null;
      app.libraryMetricDraft = null;
      setMetricLibraryView("browse");
      elements["metric-options"].replaceChildren();
      renderMetricDetail();
      elements["metric-library-summary"].textContent = `${libraryCatalog().length} metrics`;
    });
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) {
        cancelScheduledRender();
        stopFormulaPreviewAnimation({ clear: false });
        if (runtime.isBrowser && app.currentInputKind === "web-bluetooth") {
          browserLifecycleWarningPending = true;
          addActivity("Tab hidden · Android may suspend browser Bluetooth capture");
        }
      } else {
        requestRender();
        if (formulaPreviewAnimationResult && elements["formula-dialog"].open) {
          setFormulaPreviewResult(formulaPreviewAnimationResult);
        }
        if (browserLifecycleWarningPending) {
          browserLifecycleWarningPending = false;
          toast("Chrome may have paused or discarded data while this tab was hidden. Check sensor timestamps for gaps.", true);
        }
      }
    });

    const observer = new ResizeObserver(() => {
      resizeCanvas();
      requestRender();
    });
    observer.observe(elements["chart-shell"]);
  }

  async function scanDevices({ automatic = false } = {}) {
    if (app.scanning) return;
    app.scanning = true;
    elements["scan-button"].disabled = true;
    elements["scan-button"].classList.add("scanning");
    elements["scan-button"].querySelector("span").textContent = "Scanning…";
    elements["input-state"].textContent = "Scanning";
    setTopStatus(runtime.isBrowser ? "Opening the Polar Bluetooth chooser" : "Scanning for Polar and Vernier sensors", "working");
    addActivity(automatic ? "Looking for last used sensor" : "BLE scan started");

    try {
      const discovered = await runtime.scanDevices();
      const devices = [...runtime.getInputModules(), ...discovered]
        .filter((device, index, all) => all.findIndex((candidate) => candidate.id === device.id) === index);
      app.devices = devices;
      renderDevices(devices);
      const count = devices.length;
      const polarCount = devices.filter((device) => device.kind !== "mock" && device.available !== false).length;

      if (automatic && app.preferences.lastDevice) {
        const exact = devices.find((device) => device.id === app.preferences.lastDevice.id);
        const nameMatches = devices.filter((device) => device.name === app.preferences.lastDevice.name);
        const preferredDevice = exact || (nameMatches.length === 1 ? nameMatches[0] : null);
        if (preferredDevice) {
          addActivity(`Last used sensor found · ${preferredDevice.name}`);
          await connectDevice(preferredDevice, { automatic: true });
          return;
        }
        elements["input-state"].textContent = polarCount ? `${polarCount} found` : "Mock ready";
        setTopStatus("Last used sensor unavailable · choose below");
        addActivity("Last used sensor was not found");
        return;
      }

      elements["input-state"].textContent = polarCount ? `${polarCount} found` : "Mock ready";
      setTopStatus(
        app.connected
          ? "Input connected · choose another to switch"
          : polarCount
            ? "Choose a Polar H10, Vernier GDX-RB, or mock input"
            : "Recorded Polar H10 preview is available",
        app.connected ? "connected" : "idle",
      );
      addActivity(polarCount
        ? `${polarCount} compatible Bluetooth sensor${polarCount === 1 ? "" : "s"} found · mock also available`
        : "No compatible Bluetooth sensor found · offline mock remains available");
    } catch (error) {
      const message = runtime.formatError(error);
      setTopStatus("Bluetooth scan failed", "error");
      elements["input-state"].textContent = "Error";
      addActivity(message);
      toast(message, true);
    } finally {
      app.scanning = false;
      elements["scan-button"].disabled = false;
      elements["scan-button"].classList.remove("scanning");
      elements["scan-button"].querySelector("span").textContent = runtime.isBrowser ? "Choose Polar H10" : "Scan again";
    }
  }

  function renderDevices(devices) {
    if (!devices.length) {
      const empty = document.createElement("div");
      empty.className = "empty-state";
      const orbit = document.createElement("span");
      orbit.className = "empty-orbit";
      const message = document.createElement("p");
      message.textContent = "No compatible Bluetooth sensors found.";
      const hint = document.createElement("small");
      hint.textContent = "Check Bluetooth and sensor power, then scan again.";
      empty.append(orbit, message, hint);
      elements["device-list"].replaceChildren(empty);
      return;
    }

    const rows = devices.map((device) => {
      const isMock = device.kind === "mock";
      const isWebBluetooth = device.kind === "web-bluetooth";
      const isWebVernier = device.kind === "web-bluetooth-vernier";
      const isCurrent = [...app.activeSources.values()].some((source) => source.deviceId === device.id);
      const isPending = app.pendingDevice?.id === device.id;
      const isPreferred = app.preferences.lastDevice?.id === device.id;
      const button = document.createElement("button");
      button.className = `device-row${isMock ? " mock" : ""}${isWebBluetooth || isWebVernier ? " browser-bluetooth" : ""}${device.available === false ? " unavailable" : ""}${isCurrent ? " current" : ""}${isPreferred ? " preferred" : ""}`;
      button.type = "button";
      button.dataset.inputKind = isMock ? "mock" : isWebVernier ? "web-bluetooth-vernier" : isWebBluetooth ? "web-bluetooth" : device.inputKind || "polar";
      button.disabled = app.connecting || (isCurrent && !isWebVernier) || device.available === false;
      button.addEventListener("click", () => connectDevice(device));

      const icon = document.createElement("span");
      icon.className = "device-icon";
      icon.textContent = isMock ? "NK" : isWebVernier ? "GDX" : isWebBluetooth ? "BT" : device.inputKind === "vernierGoDirect" ? "GDX" : "H10";
      const copy = document.createElement("span");
      copy.className = "device-copy";
      const nameLine = document.createElement("span");
      nameLine.className = "device-name-line";
      const name = document.createElement("strong");
      name.textContent = device.name;
      nameLine.append(name);
      if (device.sourceLabel) {
        const badge = document.createElement("span");
        badge.className = "preference-badge source-badge";
        badge.textContent = device.sourceLabel;
        nameLine.append(badge);
      }
      if (isPreferred) {
        const badge = document.createElement("span");
        badge.className = "preference-badge";
        badge.textContent = "LAST USED";
        nameLine.append(badge);
      }
      const id = document.createElement("small");
      id.textContent = device.detail || device.id;
      copy.append(nameLine, id);
      const rssi = document.createElement("span");
      rssi.className = "rssi";
      rssi.textContent = isCurrent
        ? "Connected"
        : isPending
          ? "Connecting…"
          : isMock
            ? "Start demo →"
          : device.available === false
            ? "Unavailable"
          : isWebBluetooth || isWebVernier
            ? `Choose ${isWebVernier ? "GDX" : "H10"} →`
          : device.rssi == null
            ? "Connect →"
            : `${device.rssi} dBm  →`;
      button.append(icon, copy, rssi);
      return button;
    });
    elements["device-list"].replaceChildren(...rows);
  }

  async function connectDevice(device, { automatic = false } = {}) {
    if (app.connecting) return;
    const generation = ++app.connectionGeneration;
    const isMock = runtime.isMockDevice(device.id);
    const isWebBluetooth = runtime.isBrowserBluetoothDevice(device.id);
    const isWebVernier = runtime.isBrowserVernierDevice(device.id);
    app.connecting = true;
    app.pendingDevice = device;
    elements["scan-button"].disabled = true;
    elements["scan-button"].querySelector("span").textContent = isWebBluetooth || isWebVernier ? "Waiting for selection…" : "Connecting…";
    renderDevices(app.devices);
    setTopStatus(
      isMock
        ? "Starting recorded Polar H10 preview"
        : isWebBluetooth || isWebVernier
          ? "Waiting for browser Bluetooth selection"
          : automatic ? "Reconnecting to last used Polar H10" : "Connecting to Polar H10",
      "working",
    );
    elements["input-state"].textContent = "Connecting";
    elements["device-name"].textContent = device.name;
    elements["connection-detail"].textContent = isMock
      ? "Loading the checked-in anonymized recording…"
      : isWebBluetooth || isWebVernier
        ? `Choose the ${isWebVernier ? "Go Direct sensor" : "Polar H10"} in the browser permission prompt…`
      : "Opening the low-energy connection…";
    addActivity(`${isMock ? "Starting" : automatic ? "Reconnecting" : "Connecting"} ${device.name}`);

    try {
      const source = await runtime.connectDevice(device.id, (event) => handleNativeEvent(event, device));
      if (source?.id) {
        const connectedSource = app.activeSources.get(source.id);
        registerSource({
          ...source,
          ...connectedSource,
          deviceId: device.id,
          deviceName: connectedSource?.deviceName || device.name,
        });
        if (app.selectedSourceId === source.id) updateSelectedSourceUi();
      }
    } catch (error) {
      if (generation !== app.connectionGeneration) return;
      app.connecting = false;
      app.pendingDevice = null;
      elements["scan-button"].disabled = false;
      elements["scan-button"].querySelector("span").textContent = runtime.isBrowser ? "Choose Polar H10" : "Scan again";
      if ((isWebBluetooth || isWebVernier) && error?.code === "BLUETOOTH_CHOOSER_CANCELLED") {
        setTopStatus("Browser inputs ready");
        elements["input-state"].textContent = "Browser ready";
        elements["device-name"].textContent = "No sensor connected";
        elements["connection-detail"].textContent = isWebVernier
          ? "No sensor was selected. Wake the Go Direct sensor, close other Vernier apps, then choose it again."
          : "No sensor was selected. Wear the strap, close other Polar apps, then choose the H10 again.";
        addActivity("Bluetooth chooser closed without selecting a sensor");
        renderDevices(app.devices);
        return;
      }
      setTopStatus("Connection failed", "error");
      elements["input-state"].textContent = "Error";
      const message = runtime.formatError(error);
      elements["connection-detail"].textContent = message;
      addActivity(message);
      toast(message, true);
      renderDevices(app.devices);
    }
  }

  async function disconnectDevice() {
    const sourceId = app.selectedSourceId;
    if (!sourceId) return;
    try {
      const result = await runtime.disconnectDevice(sourceId);
      if (!result?.emitted && app.activeSources.has(sourceId)) {
        handleNativeEvent({
          kind: "connection", connected: false, streaming: false,
          source: app.activeSources.get(sourceId), deviceName: elements["device-name"].textContent,
          batteryPercent: null, message: "Disconnected",
        });
      }
    } catch (error) {
      toast(runtime.formatError(error), true);
    }
  }

  function handleNativeEvent(event, device = null) {
    const source = eventSource(event, device);
    if (source && event.kind !== "status" && event.kind !== "error" && event.kind !== "connection") {
      registerSource(source);
    }
    const previousBuffers = buffers;
    if (source) buffers = buffersForSource(source.id);
    audioDataLink?.capture(event);
    ingestFormulaBatch(event.formulas);
    switch (event.kind) {
      case "status":
        setTopStatus(event.message, "working");
        elements["connection-detail"].textContent = event.message;
        addActivity(event.message);
        break;
      case "connection":
        updateConnection(event, device);
        break;
      case "ecg":
        ingestEcg(event.microvolts || []);
        break;
      case "accelerometer":
        ingestAccelerometer(event.samples || []);
        break;
      case "metrics":
        ingestMetrics(event);
        break;
      case "force":
        ensureBuffer("raw_force").pushMany(event.values || []);
        app.sampleCount += (event.values || []).length;
        markTelemetryDirty();
        break;
      case "streamHealth":
        if (event.droppedBatches || event.malformedFrames) {
          addActivity(`${source?.label || "Source"}: ${event.droppedBatches || 0} dropped batches · ${event.malformedFrames || 0} malformed frames`);
        }
        break;
      case "error":
        if (event.code === "CSV_RECORDING_STOPPED") {
          elements["csv-toggle"].checked = false;
          elements["csv-detail"].textContent = event.message;
          elements["csv-detail"].classList.add("warning");
          void configureOutputs({ quiet: true });
        }
        addActivity(event.message);
        toast(event.message, true);
        break;
      default:
        break;
    }
    buffers = app.selectedSourceId ? buffersForSource(app.selectedSourceId) : previousBuffers;
    applySourceColor();
  }

  function eventSource(event, device = null) {
    if (event?.source?.id) return {
      ...event.source,
      deviceId: device?.id,
      ...(event.deviceName ? { deviceName: event.deviceName } : {}),
    };
    if (!device && !event?.simulated && event?.transport !== "web-bluetooth") return null;
    const browserPolar = event?.transport === "web-bluetooth" || device?.kind === "web-bluetooth";
    return {
      id: browserPolar ? "browser-polar-source" : "mock-source",
      slot: browserPolar ? "browser-polar-source" : "mock-source",
      label: browserPolar ? "Browser H10" : "Recorded preview",
      color: browserPolar ? "#00c2ff" : "#7c8c84",
      inputKind: browserPolar ? "web-bluetooth" : "mock",
      deviceId: device?.id,
      deviceName: event?.deviceName || device?.name,
    };
  }

  function buffersForSource(sourceId) {
    if (!sourceBuffers.has(sourceId)) sourceBuffers.set(sourceId, createBufferBank());
    return sourceBuffers.get(sourceId);
  }

  function registerSource(source, { focus = false } = {}) {
    const existing = app.activeSources.get(source.id) || {};
    app.activeSources.set(source.id, { ...existing, ...source });
    if (!app.selectedSourceId || focus) selectSource(source.id);
    else renderActiveSources();
  }

  function selectSource(sourceId) {
    if (!app.activeSources.has(sourceId)) return;
    app.selectedSourceId = sourceId;
    buffers = buffersForSource(sourceId);
    const source = app.activeSources.get(sourceId);
    app.currentInputKind = source.inputKind === "vernierGoDirect"
      ? "web-bluetooth-vernier"
      : source.inputKind || null;
    renderActiveSources();
    updateSelectedSourceUi();
    refreshTelemetry();
    renderOutputs();
    updateVisualLabels();
    requestRender();
  }

  function renderActiveSources() {
    const sources = [...app.activeSources.values()];
    const chips = sources.map((source) => {
      const chip = document.createElement("span");
      chip.className = `active-source-chip${source.id === app.selectedSourceId ? " selected" : ""}`;
      chip.style.setProperty("--source-color", source.color);
      chip.addEventListener("click", () => selectSource(source.id));
      const label = document.createElement("span");
      label.textContent = `${source.label} · ${source.deviceName || source.inputKind || "sensor"}`;
      const close = document.createElement("button");
      close.type = "button";
      close.textContent = "×";
      close.setAttribute("aria-label", `Disconnect ${source.label}`);
      close.addEventListener("click", (event) => {
        event.stopPropagation();
        void runtime.disconnectDevice(source.id);
      });
      chip.append(label, close);
      return chip;
    });
    elements["active-source-strip"].replaceChildren(...chips);
    const options = sources.map((source) => new Option(
      `${source.label} · ${source.deviceName || source.inputKind || "sensor"}`,
      source.id,
    ));
    elements["visual-device"].replaceChildren(...options);
    elements["visual-device"].value = app.selectedSourceId || "";
    elements["visual-device"].disabled = sources.length < 2;
    applySourceColor();
  }

  function applySourceColor() {
    const source = app.activeSources.get(app.selectedSourceId);
    const color = source?.color || "#9db2a7";
    elements["chart-shell"].style.setProperty("--source-color", color);
    elements["chart-shell"].classList.toggle("source-marked", Boolean(source));
    document.querySelectorAll(".raw-card, .output-card").forEach((card) => {
      card.style.setProperty("--source-color", color);
      card.classList.toggle("source-marked", Boolean(source));
    });
  }

  function selectedSourceColor(fallback) {
    return app.activeSources.get(app.selectedSourceId)?.color || fallback;
  }

  function updateSelectedSourceUi() {
    const source = app.activeSources.get(app.selectedSourceId);
    if (!source) return;
    const isVernier = source.inputKind === "vernierGoDirect";
    elements["device-name"].textContent = source.deviceName || source.label;
    const firmwareDetail = isVernier && source.firmwareVersion
      ? ` · firmware ${source.firmwareVersion}` : "";
    elements["connection-detail"].textContent = `${app.activeSources.size} source${app.activeSources.size === 1 ? "" : "s"} streaming · ${source.label}${firmwareDetail}`;
    elements["battery-value"].textContent = source.batteryPercent == null ? "—" : `${source.batteryPercent}%`;
    const sampleRate = source.samplePeriodUs > 0 ? 1_000_000 / source.samplePeriodUs : 10;
    if (isVernier && visualDefinitions.raw_force) visualDefinitions.raw_force.rate = sampleRate;
    elements["connection-metric-1-label"].textContent = isVernier ? (source.sensorName || "FORCE").toUpperCase() : "ECG";
    elements["connection-metric-1-value"].textContent = isVernier ? `${sampleRate.toFixed(sampleRate % 1 ? 1 : 0)} Hz` : "130 Hz";
    elements["connection-metric-2-label"].textContent = isVernier ? "CHANNEL" : "ACC";
    elements["connection-metric-2-value"].textContent = isVernier ? String(source.sensorNumber ?? 1) : "200 Hz";
  }

  function ingestFormulaBatch(batch) {
    if (!batch) return;
    for (const series of batch.series || []) {
      const formula = app.customFormulas.find((candidate) => candidate.id === series.formulaId);
      if (!formula) continue;
      ensureBuffer(`formula:${formula.id}`).pushMany(series.values || []);
    }
    for (const fault of batch.faults || []) {
      const key = `${fault.formulaId}:${fault.code}`;
      if (app.formulaNoticesShown.has(key)) continue;
      app.formulaNoticesShown.add(key);
      toast(`Formula stopped: ${fault.message}`, true);
    }
    for (const warning of batch.warnings || []) {
      const key = `warning:${warning}`;
      if (app.formulaNoticesShown.has(key)) continue;
      app.formulaNoticesShown.add(key);
      toast(warning, true);
    }
    if ((batch.series || []).length) markTelemetryDirty();
  }

  function updateConnection(event, device = null) {
    const source = eventSource(event, device);
    if (event.connected && source) registerSource({
      ...source,
      deviceName: event.deviceName,
      batteryPercent: event.batteryPercent,
      deviceModel: event.deviceModel,
      firmwareVersion: event.firmwareVersion,
      sensorNumber: event.sensorNumber,
      sensorName: event.sensorName,
      sensorUnit: event.sensorUnit,
      samplePeriodUs: event.samplePeriodUs,
      message: event.message,
    }, { focus: true });
    if (!event.connected && source) {
      app.activeSources.delete(source.id);
      sourceBuffers.delete(source.id);
      if (app.selectedSourceId === source.id) {
        app.selectedSourceId = app.activeSources.keys().next().value || null;
        buffers = app.selectedSourceId ? buffersForSource(app.selectedSourceId) : createBufferBank();
      }
    }
    app.connected = app.activeSources.size > 0;
    const simulated = Boolean(event.simulated || device?.kind === "mock");
    const webBluetooth = event.transport === "web-bluetooth" || device?.kind === "web-bluetooth";
    renderRuntimeContext({
      simulated: app.connected && simulated,
      transport: app.connected && webBluetooth ? "web-bluetooth" : null,
    });
    app.connecting = false;
    app.pendingDevice = null;
    elements["scan-button"].disabled = false;
    elements["scan-button"].querySelector("span").textContent = runtime.isBrowser ? "Choose Polar H10" : "Scan again";
    const selectedKind = app.activeSources.get(app.selectedSourceId)?.inputKind;
    if (app.connected) {
      const connectedDevice = device || app.devices.find((candidate) => candidate.name === event.deviceName);
      app.currentDeviceId = connectedDevice?.id || null;
      app.currentInputKind = selectedKind === "vernierGoDirect"
        ? "web-bluetooth-vernier"
        : simulated ? "mock" : webBluetooth ? "web-bluetooth" : selectedKind || "polar";
      if (connectedDevice && !simulated && !webBluetooth) {
        app.preferences = { ...app.preferences, lastDevice: { id: connectedDevice.id, name: connectedDevice.name } };
        if (!isNative) app.preferences = preferences.saveLastDevice(connectedDevice);
      }
    } else if (!app.connected) {
      app.currentDeviceId = null;
      app.currentInputKind = null;
    }
    renderDevices(app.devices);
    renderActiveSources();
    if (app.connected) updateSelectedSourceUi();
    elements["connection-card"].classList.toggle("connected", app.connected);
    elements["disconnect-button"].hidden = !app.connected;
    elements["connection-meta"].hidden = !app.connected;
    const selected = app.activeSources.get(app.selectedSourceId);
    elements["device-name"].textContent = app.connected ? selected?.deviceName || event.deviceName : "No sensor connected";
    if (!app.connected) {
      elements["connection-detail"].textContent = "Scan for nearby Polar H10 and Vernier Go Direct sensors.";
    }
    elements["battery-value"].textContent = event.batteryPercent == null ? "—" : `${event.batteryPercent}%`;
    elements["input-state"].textContent = app.connected
      ? simulated ? "Recorded preview looping" : webBluetooth ? "Browser BLE live" : "Streaming"
      : runtime.isBrowser ? "Browser ready" : "Idle";
    setTopStatus(
      app.connected
        ? simulated
          ? "Recorded H10 preview · seamless loop live"
          : webBluetooth
            ? selectedKind === "vernierGoDirect"
              ? "Go Direct connected directly to this browser tab"
              : "H10 connected directly to this browser tab"
            : "Sensor connected · streams live"
        : runtime.isBrowser ? "Browser inputs ready" : "Ready to connect",
      app.connected ? "connected" : "idle",
    );
    addActivity(app.connected ? `${event.deviceName} ${simulated ? "started" : "connected"}` : simulated ? "Recorded preview stopped" : "Sensor disconnected");
    if (!app.connected) elements["render-rate"].textContent = "Idle";
    if (elements["output-dialog"].open) {
      updateMetricFamilyUi();
      renderMetricOptions();
    }
    renderOutputs();
    if (runtime.isBrowser && browserSession) renderBrowserRecorder(browserSession.status());
    markTelemetryDirty();
  }

  function renderBrowserRecorder(status) {
    if (!runtime.isBrowser || !status) return;
    const recording = status.state === "recording";
    const stopped = status.state === "stopped";
    elements["csv-toggle"].checked = recording;
    elements["browser-export-button"].disabled = !stopped || !status.hasData;
    elements["browser-discard-button"].hidden = !stopped || !status.hasData;
    elements["browser-recorder-status"].textContent = recording
      ? "REC"
      : status.stopReason === "capacity" ? "FULL" : stopped ? "FILE" : "READY";
    elements["browser-recorder-status"].classList.toggle("recording", recording);
    elements["browser-recorder-status"].classList.toggle("full", status.stopReason === "capacity");
    elements["browser-recorder-count"].textContent = recording
      ? `${status.rowCount.toLocaleString()} / ${status.maxRows.toLocaleString()} rows captured`
      : status.stopReason === "capacity"
        ? `File limit reached at ${status.rowCount.toLocaleString()} rows · download now`
        : stopped
          ? `${status.rowCount.toLocaleString()} rows ready to download`
          : `Ready · up to ${status.maxRows.toLocaleString()} rows per file`;
    elements["csv-detail"].textContent = recording
      ? `Recording every incoming browser row · ${status.rowCount.toLocaleString()} captured`
      : stopped && status.hasData
        ? `${status.rowCount.toLocaleString()} rows waiting for download`
        : "All received raw data and produced metrics · 300,000-row limit";
    if (status.stopReason === "capacity" && !browserRecorderCapacityNotified) {
      browserRecorderCapacityNotified = true;
      toast("Browser recording reached its safe file limit. Download it before starting another.", true);
    } else if (status.stopReason !== "capacity") {
      browserRecorderCapacityNotified = false;
    }
  }

  function renderAudioOutput(status) {
    if (!status) return;
    const bitRate = status.bitRate ? `${(status.bitRate / 1000).toFixed(2)} kbit/s` : "22.05 kbit/s";
    elements["audio-detail"].textContent = status.error
      ? status.error
      : status.enabled
        ? `Sending ${bitRate} stereo PCM · ${status.frameCount.toLocaleString()} frames`
        : "CRC-checked stereo data modem · cable or digital recording";
    elements["audio-detail"].classList.toggle("warning", Boolean(status.error));
  }

  function ingestEcg(values) {
    buffers.raw_ecg.pushMany(values);
    app.sampleCount += values.length;
    markTelemetryDirty();
  }

  function ingestAccelerometer(samples) {
    if (!samples.length) return;
    for (const sample of samples) {
      const x = Number(sample.xMg ?? sample.x_mg ?? 0);
      const y = Number(sample.yMg ?? sample.y_mg ?? 0);
      const z = Number(sample.zMg ?? sample.z_mg ?? 0);
      buffers.acc_x.push(x);
      buffers.acc_y.push(y);
      buffers.acc_z.push(z);
      ensureBuffer("acc_magnitude").push(visualValue("acc_magnitude", Math.hypot(x, y, z) / 1000));
    }
    app.sampleCount += samples.length;
    markTelemetryDirty();
  }

  function ingestMetrics(event) {
    if (Array.isArray(event.values)) {
      for (const metric of event.values) {
        // ACC magnitude is already calculated while unpacking raw axes above;
        // avoid drawing every native value twice.
        if (metric.id !== "acc_magnitude") {
          ensureBuffer(metric.id).push(visualValue(metric.id, Number(metric.value)));
        }
      }
      markTelemetryDirty();
      return;
    }
    // Backward compatibility for early v0.1 event payloads.
    ensureBuffer("heart_rate").push(visualValue("heart_rate", event.heartRateBpm ?? event.heart_rate_bpm));
    for (const value of event.rrIntervalsMs ?? event.rr_intervals_ms ?? []) {
      ensureBuffer("rr_interval").push(visualValue("rr_interval", value));
    }
    const rmssd = event.rmssdMs ?? event.rmssd_ms;
    if (rmssd != null) ensureBuffer("rmssd").push(visualValue("rmssd", rmssd));
    markTelemetryDirty();
  }

  function markTelemetryDirty() {
    telemetryDirty = true;
    requestRender();
  }

  function refreshTelemetry() {
    elements["raw-ecg-value"].textContent = formatValue(buffers.raw_ecg.latest(), 0);
    elements["raw-acc-x"].textContent = formatValue(buffers.acc_x.latest(), 0);
    elements["raw-acc-y"].textContent = formatValue(buffers.acc_y.latest(), 0);
    elements["raw-acc-z"].textContent = formatValue(buffers.acc_z.latest(), 0);
    elements["raw-force-value"].textContent = formatValue(ensureBuffer("raw_force").latest(), 3);
    updateSparkline();
    updateSampleCounter();
  }

  function updateSampleCounter() {
    elements["sample-counter"].textContent = `${app.sampleCount.toLocaleString()} samples`;
  }

  function installCatalogVisuals() {
    const palette = {
      "Raw signals": "#d85151", "ECG features": "#b24e68", "Heart rate": "#d85151",
      "HRV & relaxation": "#168259", Coherence: "#6c62a8", Breathing: "#3b78aa",
      "Breathing dynamics": "#a66d19", "Excitation (experimental)": "#b94b40",
    };
    for (const metric of app.catalog) {
      if (!visualDefinitions[metric.id]) {
        visualDefinitions[metric.id] = {
          label: metric.label, unit: metric.unit, rate: Number(metric.rateHz) || 1,
          color: palette[metric.category] || "#168259",
          symmetric: metric.id === "breathing_phase" || /^ecg_(mean|sd)$/.test(metric.id),
        };
      }
      ensureBuffer(metric.id);
    }
  }

  function ensureBuffer(id) {
    if (!buffers[id]) buffers[id] = new RingBuffer();
    return buffers[id];
  }

  function svgElement(name, attributes = {}) {
    const element = document.createElementNS(svgNamespace, name);
    for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, String(value));
    return element;
  }

  function createMetricPreview(metric, { animated = false } = {}) {
    const data = metricPreviewData(metric);
    const figure = document.createElement("figure");
    figure.className = "metric-preview metric-preview-large";
    figure.dataset.metricId = metric.id;
    if (!data?.channels?.length) {
      figure.classList.add("metric-preview-missing");
      figure.textContent = "Preview unavailable";
      return figure;
    }

    const [, , width, height] = metricPreviews.viewBox;
    figure.dataset.minimum = String(data.minimum);
    figure.dataset.maximum = String(data.maximum);
    figure.dataset.durationSeconds = String(data.durationSeconds);

    const svg = svgElement("svg", {
      class: "metric-preview-svg",
      viewBox: `0 0 ${width} ${height}`,
      preserveAspectRatio: "none",
      role: "img",
      "aria-label": `Recorded Polar H10 outcome preview of ${metric.label}`,
    });
    const title = svgElement("title");
    title.textContent = `${metric.label}: recorded Polar H10 outcome preview`;
    svg.append(title, svgElement("rect", { class: "metric-preview-background", width, height, rx: 8 }));
    for (const fraction of [0.25, 0.5, 0.75]) {
      svg.append(svgElement("line", {
        class: "metric-preview-grid",
        x1: 0,
        y1: (height * fraction).toFixed(2),
        x2: width,
        y2: (height * fraction).toFixed(2),
      }));
    }

    const clipId = `metric-preview-clip-${metricPreviewSequence += 1}`;
    const definitions = svgElement("defs");
    const clipPath = svgElement("clipPath", { id: clipId });
    clipPath.append(svgElement("rect", { width, height, rx: 8 }));
    definitions.append(clipPath);
    svg.append(definitions);
    const group = svgElement("g", { "clip-path": `url(#${clipId})` });
    for (const channel of data.channels) {
      for (const offset of animated ? [0, width] : [0]) {
        group.append(svgElement("path", {
          class: "metric-preview-line",
          d: channel.path || previewPath(channel.values, data.minimum, data.maximum, width, height, metric.id === "breathing_phase"),
          stroke: channel.color,
          "stroke-width": 2,
          transform: offset ? `translate(${offset} 0)` : "",
        }));
      }
    }
    if (animated && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      group.classList.add("metric-preview-loop");
      const animation = svgElement("animateTransform", {
        attributeName: "transform",
        type: "translate",
        from: "0 0",
        to: `${-width} 0`,
        dur: "8s",
        repeatCount: "indefinite",
      });
      group.append(animation);
    }
    svg.append(group);
    figure.append(svg);
    return figure;
  }

  function metricPreviewData(metric) {
    const stored = metricPreviews?.metrics?.[metric.id];
    if (!stored?.channels?.length) return stored;
    const settings = app.libraryMetricDraft || metricOptionFor(metric.id, { forSelection: true });

    if (previewRecording && metric.formulaTemplate) {
      try {
        const result = formulaPreview.preview(previewRecording, {
          id: `preview-${metric.id}`,
          name: metric.streamSuffix,
          unit: metric.unit,
          source: metric.formulaSource,
          expression: previewFormulaExpression(metric, settings),
          enabled: true,
        }, {
          displaySeconds: settings.displayWindowSeconds,
          normalization: settings.normalization,
          windowSeconds: settings.windowSeconds,
        });
        const values = result.output.map((sample) => sample.value);
        if (values.length) return previewPayload([{
          label: "Formula output",
          color: stored.channels[0].color,
          values,
          step: result.outputStep,
        }], result.output.at(-1).time - result.output[0].time);
      } catch {
        // The checked-in recorded derivation remains available while a draft
        // setting or formula is temporarily incomplete.
      }
    }

    const channels = stored.channels.map((channel) => ({
      label: channel.label,
      color: channel.color,
      values: transformPreviewValues(channel.values || [], stored.durationSeconds, settings),
      step: metric.id === "breathing_phase",
    }));
    const duration = Math.min(Number(stored.durationSeconds) || 0, Number(settings.displayWindowSeconds) || Number(stored.durationSeconds) || 0);
    return previewPayload(channels, duration);
  }

  function previewFormulaExpression(metric, settings) {
    if (metric.id !== "acc_breathing_magnitude" && metric.id !== "breathing_phase") return metric.formulaTemplate;
    const breathing = settings.processing?.breathing || defaultBreathingSettings();
    const [axisX, axisY, axisZ] = breathing.axes.map((enabled) => String(Boolean(enabled)));
    const smoothing = Number(breathing.smoothingWindowSeconds) || 0.75;
    const invert = String(Boolean(breathing.invertDirection));
    if (metric.id === "breathing_phase") {
      return `breathing_phase(x, y, z, ${axisX}, ${axisY}, ${axisZ}, ${smoothing}, ${Number(breathing.sensitivity) || 0.60}, ${invert})`;
    }
    const normalize = String(settings.normalization !== "none");
    return `breathing_magnitude(x, y, z, ${axisX}, ${axisY}, ${axisZ}, ${smoothing}, ${normalize}, ${invert})`;
  }

  function transformPreviewValues(values, durationSeconds, settings) {
    if (!values.length) return [];
    const visible = Math.max(1, Number(settings.displayWindowSeconds) || durationSeconds);
    const count = Math.max(2, Math.min(values.length, Math.ceil(values.length * visible / Math.max(visible, durationSeconds))));
    const source = values.map(Number);
    if (settings.normalization === "none") return source.slice(source.length - count);
    if (settings.normalization === "session") {
      const low = Math.min(...values);
      const high = Math.max(...values);
      return source.map((value) => previewMinMax(value, low, high)).slice(source.length - count);
    }
    const windowCount = Math.max(2, Math.ceil(values.length * (Number(settings.windowSeconds) || 60) / Math.max(1, durationSeconds)));
    const transformed = source.map((value, index) => {
      const window = source.slice(Math.max(0, index - windowCount + 1), index + 1);
      return previewMinMax(value, Math.min(...window), Math.max(...window));
    });
    return transformed.slice(transformed.length - count);
  }

  function previewMinMax(value, low, high) {
    return Math.abs(high - low) < Number.EPSILON ? 0.5 : Math.max(0, Math.min(1, (value - low) / (high - low)));
  }

  function previewPayload(channels, durationSeconds) {
    const loopedChannels = channels.map((channel) => ({
      ...channel,
      values: closeMetricPreviewValues(channel.values, channel.step),
    }));
    const all = loopedChannels.flatMap((channel) => channel.values).filter(Number.isFinite);
    if (!all.length) return { channels: [], minimum: 0, maximum: 0, durationSeconds: 0 };
    let minimum = Math.min(...all);
    let maximum = Math.max(...all);
    if (Math.abs(maximum - minimum) < Number.EPSILON) {
      minimum -= 1;
      maximum += 1;
    }
    return {
      channels: loopedChannels.map((channel) => ({
        ...channel,
        path: previewPath(channel.values, minimum, maximum, 240, 72, channel.step),
      })),
      minimum,
      maximum,
      durationSeconds: Math.max(0.1, Number(durationSeconds) || 0).toLocaleString(undefined, { maximumFractionDigits: 1 }),
    };
  }

  function closeMetricPreviewValues(values, stepped = false) {
    if (!stepped) {
      return previewFixtureApi.circularizeSignal(values, Math.max(4, Math.round(values.length * 0.14)));
    }
    const result = Array.from(values, Number);
    if (result.length > 1) result[result.length - 1] = result[0];
    return result;
  }

  function previewPath(values, minimum, maximum, width = 240, height = 72, step = false) {
    const span = Math.max(Number.EPSILON, maximum - minimum);
    return values.map((value, index) => {
      const x = index / Math.max(1, values.length - 1) * width;
      const y = 7 + (1 - (value - minimum) / span) * (height - 14);
      if (!index) return `M${x.toFixed(1)},${y.toFixed(1)}`;
      if (step) {
        const previous = values[index - 1];
        const previousY = 7 + (1 - (previous - minimum) / span) * (height - 14);
        return `L${x.toFixed(1)},${previousY.toFixed(1)}L${x.toFixed(1)},${y.toFixed(1)}`;
      }
      return `L${x.toFixed(1)},${y.toFixed(1)}`;
    }).join("");
  }

  function createMetricPreviewPanel(metric) {
    const section = document.createElement("section");
    section.className = "metric-preview-panel";
    const title = document.createElement("h4");
    title.textContent = "Animated output preview";
    section.append(title, createMetricPreview(metric, { animated: true }));
    return section;
  }

  function configuredBreathingSettings(metricOptions) {
    for (const id of breathingOutputIds) {
      const processing = metricOptions?.[id]?.processing || {};
      const stored = processing.breathing || processing.breathingPhase;
      if (stored) return { ...defaultBreathingSettings(), ...structuredClone(stored) };
    }
    return defaultBreathingSettings();
  }

  function libraryCatalog() {
    if (app.metricFamily === "acc") {
      return app.catalog.filter((metric) => accLibraryIds.has(metric.id));
    }
    return app.catalog.filter((metric) => (
      !accLibraryIds.has(metric.id)
      && metric.category !== "Breathing"
      && metric.category !== "Breathing dynamics"
    ));
  }

  function setMetricLibraryView(view) {
    const nextView = view === "detail" ? "detail" : "browse";
    elements["output-dialog"].dataset.mobileView = nextView;
    if (nextView === "detail") elements["metric-detail"].scrollTop = 0;
  }

  function updateMetricFamilyUi() {
    elements["output-dialog"].dataset.family = app.metricFamily;
    for (const button of elements["metric-family-toggle"].querySelectorAll("button[data-family]")) {
      const active = button.dataset.family === app.metricFamily;
      button.classList.toggle("active", active);
      button.setAttribute("aria-pressed", String(active));
    }
    const acc = app.metricFamily === "acc";
    elements["metric-family-context"].textContent = acc
      ? "Experimental ACC research outputs"
      : "ECG-first outputs";
    elements["metric-family-note"].textContent = acc
      ? "ACC breathing outputs are unvalidated. Keep still and compare them with a reference respiratory sensor."
      : app.currentInputKind === "web-bluetooth"
        ? "Browser H10 input exposes raw ECG plus HR/RR here. Other derived ECG processors remain desktop-only."
        : "Start with ECG for the signal the H10 is designed to measure; interpretation limits still apply.";
    elements["metric-search"].placeholder = acc
      ? "Search raw motion or breathing…"
      : "Search ECG, heart rate, HRV…";
  }

  function renderMetricFilters() {
    const categories = ["All", ...new Set(libraryCatalog().map((metric) => metric.category))];
    const buttons = categories.map((category) => {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.category = category;
      button.className = category === app.metricFilter ? "active" : "";
      button.textContent = category;
      button.setAttribute("aria-pressed", String(category === app.metricFilter));
      return button;
    });
    elements["metric-filters"].replaceChildren(...buttons);
  }

  function renderMetricOptions() {
    const familyCatalog = libraryCatalog();
    const visible = familyCatalog.filter((metric) => {
      const categoryMatches = app.metricFilter === "All" || metric.category === app.metricFilter;
      const haystack = `${metric.label} ${metric.detail} ${metric.category} ${metric.keywords || ""}`.toLowerCase();
      return categoryMatches && (!app.metricSearch || haystack.includes(app.metricSearch));
    });
    const options = visible.map((metric) => {
      const support = runtime.outputSupport(metric.id, app.currentInputKind);
      const option = document.createElement("button");
      option.type = "button";
      option.dataset.metricId = metric.id;
      option.className = `metric-option${app.selectedMetricId === metric.id ? " selected" : ""}${support.supported ? "" : " unavailable"}`;
      option.setAttribute("aria-pressed", String(app.selectedMetricId === metric.id));
      const mark = document.createElement("span");
      mark.textContent = app.metricFamily === "acc" ? "ACC" : "ECG";
      const copy = document.createElement("span");
      copy.className = "metric-option-copy";
      const heading = document.createElement("span");
      heading.className = "metric-option-heading";
      const name = document.createElement("strong");
      name.textContent = metric.label;
      const evidence = document.createElement("em");
      evidence.textContent = metric.evidence || "research metric";
      heading.append(name, evidence);
      const detail = document.createElement("small");
      detail.textContent = `${metric.detail} · ${metric.unit} · _${metric.streamSuffix}`;
      copy.append(heading, detail);
      const state = document.createElement("span");
      state.className = app.outputs.has(metric.id)
        ? "metric-added"
        : support.supported ? "metric-chevron" : "metric-unavailable";
      state.textContent = app.outputs.has(metric.id)
        ? "ADDED"
        : support.supported ? "›" : "DESKTOP";
      option.addEventListener("click", () => {
        app.selectedMetricId = metric.id;
        app.libraryMetricDraft = structuredClone(metricOptionFor(metric.id, { forSelection: true }));
        for (const candidate of elements["metric-options"].querySelectorAll(".metric-option")) {
          const selected = candidate.dataset.metricId === metric.id;
          candidate.classList.toggle("selected", selected);
          candidate.setAttribute("aria-pressed", String(selected));
        }
        renderMetricDetail();
        setMetricLibraryView("detail");
      });
      option.append(mark, copy, state);
      return option;
    });
    if (!options.length) {
      const empty = document.createElement("p");
      empty.className = "metric-library-empty";
      empty.textContent = "No metrics match this filter.";
      elements["metric-options"].replaceChildren(empty);
    } else {
      elements["metric-options"].replaceChildren(...options);
    }
    elements["metric-library-summary"].textContent = `${visible.length} of ${familyCatalog.length} ${app.metricFamily.toUpperCase()} metrics`;
    renderMetricDetail();
  }

  function renderMetricDetail() {
    const metric = app.catalog.find((candidate) => candidate.id === app.selectedMetricId);
    const save = elements["save-metric-output"];
    const status = elements["dialog-output-status"];
    if (!metric) {
      const empty = document.createElement("div");
      empty.className = "metric-detail-empty";
      const kicker = document.createElement("span");
      kicker.textContent = "SELECT A METRIC";
      const title = document.createElement("strong");
      title.textContent = "Scientific context appears here";
      const copy = document.createElement("p");
      copy.textContent = "Nothing is added until you review a metric and press Save output.";
      empty.append(kicker, title, copy);
      elements["metric-detail"].replaceChildren(empty);
      save.disabled = true;
      save.textContent = "Save output";
      status.textContent = "Select one metric to inspect";
      return;
    }
    if (breathingOutputIds.has(metric.id) && !app.libraryMetricDraft) {
      app.libraryMetricDraft = structuredClone(metricOptionFor(metric.id, { forSelection: true }));
    }

    const article = document.createElement("article");
    const header = document.createElement("header");
    const category = document.createElement("p");
    category.className = "metric-detail-category";
    category.textContent = metric.category;
    const title = document.createElement("h3");
    title.textContent = metric.label;
    header.append(category, title);

    const summary = document.createElement("section");
    summary.className = "metric-scientific-summary";
    const summaryTitle = document.createElement("h4");
    summaryTitle.textContent = "Scientific summary";
    const summaryCopy = document.createElement("p");
    summaryCopy.textContent = metric.explainer;
    summary.append(summaryTitle, summaryCopy);

    const source = document.createElement("section");
    source.className = "metric-sources";
    const sourceTitle = document.createElement("h4");
    sourceTitle.textContent = "Sources";
    const sourceList = document.createElement("ul");
    sourceList.className = "metric-source-list";
    const citations = (Array.isArray(metric.sources) && metric.sources.length
      ? metric.sources
      : [{ label: metric.citationLabel, url: metric.citationUrl }]).slice(0, 3);
    citations.forEach((entry, index) => {
      const item = document.createElement("li");
      const citation = document.createElement("a");
      citation.href = isNative ? `#citation-${metric.id}-${index + 1}` : entry.url;
      if (!isNative) {
        citation.target = "_blank";
        citation.rel = "noreferrer";
      }
      citation.textContent = `${entry.label} ↗`;
      citation.addEventListener("click", (event) => {
        event.preventDefault();
        runtime.openMetricCitation(metric.id, entry.url)
          .catch((error) => toast(runtime.formatError(error), true));
      });
      item.append(citation);
      sourceList.append(item);
    });
    source.append(sourceTitle, sourceList);

    article.append(header, createMetricPreviewPanel(metric), summary, source);
    elements["metric-detail"].replaceChildren(article);
    const alreadyAdded = app.outputs.has(metric.id);
    const support = runtime.outputSupport(metric.id, app.currentInputKind);
    const invalidAxes = breathingOutputIds.has(metric.id)
      && selectedAxisCount(app.libraryMetricDraft.processing.breathing.axes) < 2;
    const invalidBounds = breathingOutputIds.has(metric.id)
      && app.libraryMetricDraft.processing.breathing.upperQuantile
        - app.libraryMetricDraft.processing.breathing.lowerQuantile < 0.10;
    save.disabled = alreadyAdded || invalidAxes || invalidBounds || !support.supported;
    save.textContent = alreadyAdded ? "Already added" : support.supported ? "Save output" : "Desktop only";
    status.textContent = alreadyAdded
      ? `${metric.label} is already in Output`
      : !support.supported
        ? support.reason
      : invalidAxes
        ? "Choose at least two axes"
        : invalidBounds ? "Keep at least 0.10 between quantile bounds" : `Ready to add ${metric.label}`;
  }

  function selectedAxisCount(axes) {
    return Array.isArray(axes) ? axes.filter(Boolean).length : 0;
  }

  function breathingDraftIsInvalid(breathing) {
    return selectedAxisCount(breathing.axes) < 2
      || breathing.upperQuantile - breathing.lowerQuantile < 0.10;
  }

  async function openFormulaLab(metric = null, formulaId = null) {
    try {
      await ensurePreviewRecording();
    } catch (error) {
      toast(`Recorded formula preview unavailable: ${error.message || error}`, true);
    }
    renderFormulaTemplates();
    renderFormulaSavedList();
    if (!elements["formula-dialog"].open) elements["formula-dialog"].showModal();
    if (metric?.formulaTemplate) {
      editFormulaDraft(normalizeFormulaDraft({
        name: `${metric.streamSuffix}_custom`,
        source: metric.formulaSource,
        expression: previewFormulaExpression(metric, metricOptionFor(metric.id, { forSelection: true })),
        unit: metric.unit,
      }));
    } else if (formulaId) {
      editFormulaDraft(app.customFormulas.find((formula) => formula.id === formulaId));
    } else {
      editFormulaDraft(app.customFormulas[0] || normalizeFormulaDraft());
    }
  }

  function editFormulaDraft(value = null) {
    const formula = normalizeFormulaDraft(value || {});
    app.editingFormulaId = app.customFormulas.some((candidate) => candidate.id === formula.id) ? formula.id : null;
    app.formulaDraft = structuredClone(formula);
    elements["formula-name"].value = formula.name;
    elements["formula-source"].value = formula.source;
    elements["formula-unit"].value = formula.unit;
    elements["formula-expression"].value = formula.expression;
    elements["delete-custom-formula"].hidden = !app.editingFormulaId;
    elements["formula-dialog-title"].textContent = app.editingFormulaId ? `Edit ${formula.name}` : "New custom output";
    renderFormulaKeyboard();
    renderFormulaSavedList();
    scheduleFormulaPreview();
  }

  function readFormulaDraft() {
    return normalizeFormulaDraft({
      id: app.formulaDraft?.id,
      name: elements["formula-name"].value,
      source: elements["formula-source"].value,
      unit: elements["formula-unit"].value,
      expression: elements["formula-expression"].value,
      enabled: true,
    });
  }

  function renderFormulaSavedList() {
    const rows = app.customFormulas.map((formula) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = formula.id === app.editingFormulaId ? "active" : "";
      const name = document.createElement("strong");
      name.textContent = formula.name;
      const expression = document.createElement("span");
      expression.textContent = formula.expression;
      button.append(name, expression);
      button.addEventListener("click", () => editFormulaDraft(formula));
      return button;
    });
    if (!rows.length) {
      const empty = document.createElement("p");
      empty.textContent = "No custom outputs saved yet.";
      elements["formula-saved-list"].replaceChildren(empty);
    } else {
      elements["formula-saved-list"].replaceChildren(...rows);
    }
  }

  function renderFormulaTemplates() {
    const buttons = app.catalog.filter((metric) => metric.formulaTemplate).map((metric) => {
      const button = document.createElement("button");
      button.type = "button";
      const name = document.createElement("strong");
      name.textContent = metric.label;
      const expression = document.createElement("code");
      expression.textContent = metric.formulaTemplate;
      button.append(name, expression);
      button.title = `Load ${metric.label}, its expression, source, unit, and recorded preview`;
      button.addEventListener("click", () => editFormulaDraft(normalizeFormulaDraft({
        name: `${metric.streamSuffix}_custom`,
        source: metric.formulaSource,
        expression: previewFormulaExpression(metric, metricOptionFor(metric.id, { forSelection: true })),
        unit: metric.unit,
      })));
      return button;
    });
    elements["formula-template-buttons"].replaceChildren(...buttons);
  }

  function renderFormulaKeyboard() {
    const source = elements["formula-source"].value;
    const labels = { variables: "Variables", common: "Operators", functions: "Functions" };
    const groups = Object.entries(formulaPreview.keypad(source)).map(([groupName, entries]) => {
      const section = document.createElement("section");
      const title = document.createElement("span");
      title.textContent = labels[groupName];
      const keys = document.createElement("div");
      for (const entry of entries) {
        const button = document.createElement("button");
        button.type = "button";
        button.textContent = entry.label;
        button.title = entry.title;
        button.setAttribute("aria-label", `${entry.label}: ${entry.title}`);
        button.addEventListener("click", () => insertFormulaText(entry));
        keys.append(button);
      }
      section.append(title, keys);
      return section;
    });
    elements["formula-keyboard"].replaceChildren(...groups);
  }

  function insertFormulaText(entry) {
    const control = elements["formula-expression"];
    const start = control.selectionStart ?? control.value.length;
    const end = control.selectionEnd ?? start;
    const selected = control.value.slice(start, end);
    let insertion = entry.insert;
    if (selected && insertion.endsWith("()")) insertion = `${insertion.slice(0, -1)}${selected})`;
    control.setRangeText(insertion, start, end, "end");
    if (!selected && entry.cursorBack) {
      const cursor = Math.max(0, control.selectionStart - entry.cursorBack);
      control.setSelectionRange(cursor, cursor);
    }
    control.focus();
    scheduleFormulaPreview();
  }

  function scheduleFormulaPreview() {
    window.clearTimeout(app.formulaPreviewTimer);
    app.formulaPreviewTimer = window.setTimeout(() => void renderFormulaPreview(), 100);
  }

  async function renderFormulaPreview() {
    const formula = readFormulaDraft();
    app.formulaDraft = formula;
    const status = elements["formula-validation-status"];
    try {
      const result = formulaPreview.preview(previewRecording, formula, { displaySeconds: 12 });
      setFormulaPreviewResult(result);
      elements["formula-preview-current"].textContent = `${formatValue(result.current, 3)} ${formula.unit}`;
      elements["formula-preview-note"].textContent = result.note;
      status.textContent = `Checking native formula · variables: ${formulaSources[formula.source].variables}`;
      const validation = await runtime.validateCustomFormula(formula);
      if (app.formulaDraft?.id !== formula.id || app.formulaDraft.expression !== formula.expression) return;
      const stateCost = Number(validation.stateSamples || 0);
      status.textContent = `Valid · variables ${validation.allowedVariables.join(", ")}${stateCost ? ` · ${stateCost.toLocaleString()} retained samples` : ""}`;
      elements["save-custom-formula"].disabled = false;
    } catch (error) {
      setFormulaPreviewResult(null);
      elements["formula-preview-current"].textContent = "—";
      elements["formula-preview-note"].textContent = error.message || runtime.formatError(error);
      status.textContent = `Fix formula · ${error.message || runtime.formatError(error)}`;
      elements["save-custom-formula"].disabled = true;
    }
  }

  function setFormulaPreviewResult(result) {
    stopFormulaPreviewAnimation({ clear: false });
    formulaPreviewAnimationResult = result;
    formulaPreviewAnimationStartedAt = performance.now();
    formulaPreviewLastDrawAt = 0;
    const canvas = elements["formula-preview-canvas"];
    const animate = Boolean(
      result
      && elements["formula-dialog"].open
      && !document.hidden
      && !window.matchMedia("(prefers-reduced-motion: reduce)").matches
    );
    canvas.dataset.looping = String(animate);
    drawFormulaPreview(canvas, result, 0);
    if (animate) formulaPreviewAnimationId = window.requestAnimationFrame(animateFormulaPreview);
  }

  function animateFormulaPreview(now) {
    if (!formulaPreviewAnimationResult || document.hidden || !elements["formula-dialog"].open) {
      formulaPreviewAnimationId = 0;
      elements["formula-preview-canvas"].dataset.looping = "false";
      return;
    }
    if (now - formulaPreviewLastDrawAt >= renderIntervalMs) {
      const phase = ((now - formulaPreviewAnimationStartedAt) % 8000) / 8000;
      drawFormulaPreview(elements["formula-preview-canvas"], formulaPreviewAnimationResult, phase);
      formulaPreviewLastDrawAt = now;
    }
    formulaPreviewAnimationId = window.requestAnimationFrame(animateFormulaPreview);
  }

  function stopFormulaPreviewAnimation({ clear = true } = {}) {
    if (formulaPreviewAnimationId) window.cancelAnimationFrame(formulaPreviewAnimationId);
    formulaPreviewAnimationId = 0;
    elements["formula-preview-canvas"].dataset.looping = "false";
    if (clear) formulaPreviewAnimationResult = null;
  }

  function drawFormulaPreview(canvas, result, phase = 0) {
    const width = Math.max(1, canvas.clientWidth);
    const height = Math.max(1, canvas.clientHeight);
    const ratio = Math.min(2, window.devicePixelRatio || 1);
    const pixelWidth = Math.round(width * ratio);
    const pixelHeight = Math.round(height * ratio);
    if (canvas.width !== pixelWidth) canvas.width = pixelWidth;
    if (canvas.height !== pixelHeight) canvas.height = pixelHeight;
    const context = canvas.getContext("2d");
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    context.strokeStyle = "#dfe6e1";
    for (let row = 1; row < 3; row += 1) {
      context.beginPath();
      context.moveTo(0, row * height / 3);
      context.lineTo(width, row * height / 3);
      context.stroke();
    }
    if (!result) return;
    const offset = -Math.max(0, Math.min(1, phase)) * width;
    for (const copyOffset of [offset, offset + width]) {
      drawFormulaSeries(context, result.input, width, height, "#9aa8a0", copyOffset);
      drawFormulaSeries(context, result.output, width, height, "#168259", copyOffset, result.outputStep);
    }
  }

  function drawFormulaSeries(context, samples, width, height, color, offsetX = 0, stepped = false) {
    const clean = samples.filter((sample) => Number.isFinite(sample.value));
    if (!clean.length) return;
    let low = Math.min(...clean.map((sample) => sample.value));
    let high = Math.max(...clean.map((sample) => sample.value));
    if (Math.abs(high - low) < Number.EPSILON) { low -= 1; high += 1; }
    const first = clean[0].time;
    const duration = Math.max(Number.EPSILON, clean.at(-1).time - first);
    const stride = Math.max(1, Math.floor(clean.length / Math.max(1, width * 1.5)));
    const indices = [];
    for (let index = 0; index < clean.length; index += stride) indices.push(index);
    if (indices.at(-1) !== clean.length - 1) indices.push(clean.length - 1);
    context.beginPath();
    indices.forEach((index, drawIndex) => {
      const sample = clean[index];
      const x = offsetX + (sample.time - first) / duration * width;
      const y = height - 5 - (sample.value - low) / (high - low) * (height - 10);
      if (!drawIndex) {
        context.moveTo(x, y);
      } else if (stepped) {
        const previous = clean[indices[drawIndex - 1]];
        const previousY = height - 5 - (previous.value - low) / (high - low) * (height - 10);
        context.lineTo(x, previousY);
        context.lineTo(x, y);
      } else {
        context.lineTo(x, y);
      }
    });
    context.strokeStyle = color;
    context.lineWidth = color === "#168259" ? 1.7 : 1;
    context.stroke();
  }

  async function saveCustomFormula() {
    const draft = readFormulaDraft();
    try {
      const validation = await runtime.validateCustomFormula(draft);
      const normalized = normalizeFormulaDraft(validation.normalized || draft);
      const nameKey = normalized.name.toLowerCase();
      if (app.customFormulas.some((formula) => formula.id !== normalized.id && formula.name.toLowerCase() === nameKey)) {
        throw new Error("Custom output names must be unique.");
      }
      if (app.catalog.some((metric) => metric.streamSuffix.toLowerCase() === nameKey)) {
        throw new Error("That name conflicts with a built-in output suffix.");
      }
      const index = app.customFormulas.findIndex((formula) => formula.id === normalized.id);
      if (index >= 0) app.customFormulas[index] = normalized;
      else app.customFormulas.push(normalized);
      installCustomFormulaVisuals();
      renderOutputs();
      await configureOutputs();
      elements["formula-dialog"].close();
      toast(`${normalized.name} added as ${customStreamName(normalized)}`);
    } catch (error) {
      toast(error.message || runtime.formatError(error), true);
    }
  }

  function deleteCustomFormula() {
    if (!app.editingFormulaId) return;
    const formula = app.customFormulas.find((candidate) => candidate.id === app.editingFormulaId);
    app.customFormulas = app.customFormulas.filter((candidate) => candidate.id !== app.editingFormulaId);
    if (app.selectedVisual === `formula:${app.editingFormulaId}`) app.selectedVisual = "raw_ecg";
    renderOutputs();
    void configureOutputs();
    elements["formula-dialog"].close();
    toast(`${formula?.name || "Custom output"} removed`);
  }

  function installCustomFormulaVisuals() {
    for (const formula of app.customFormulas) {
      const id = `formula:${formula.id}`;
      visualDefinitions[id] = {
        label: formula.name,
        unit: formula.unit,
        rate: formula.source === "ecg" ? 130 : formula.source === "accelerometer" ? 200 : 1,
        color: formulaSources[formula.source].color,
        formulaId: formula.id,
      };
      ensureBuffer(id);
    }
  }

  function renderOutputs() {
    const byId = new Map(app.catalog.map((metric) => [metric.id, metric]));
    const cards = [...app.outputs].map((id) => {
      const metric = byId.get(id);
      if (!metric) return null;
      const support = runtime.outputSupport(id, app.currentInputKind);
      const card = document.createElement("article");
      card.className = `output-card${metric.raw ? " raw-output-card" : ""}${support.supported ? "" : " unavailable"}`;
      const header = document.createElement("header");
      const identity = document.createElement("span");
      const label = document.createElement("strong");
      label.textContent = metric.label;
      const stream = document.createElement("small");
      stream.textContent = streamOutputName(metric, elements["stream-name"].value);
      identity.append(label, stream);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.setAttribute("aria-label", `Remove ${metric.label}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        app.outputs.delete(id);
        delete app.metricOptions[id];
        renderOutputs();
        configureOutputs();
      });
      header.append(identity, remove);
      card.append(header);

      const controls = document.createElement("div");
      controls.className = "metric-controls";
      const options = metricOptionFor(id);
      const summary = document.createElement("span");
      summary.className = "module-summary";
      const scaling = options.normalization === "slidingWindow"
        ? `0–1 / ${options.windowSeconds}s`
        : options.normalization === "session" ? "0–1 / whole run" : "original scale";
      const axes = breathingOutputIds.has(id)
        ? options.processing.breathing.axes.map((enabled, index) => enabled ? ["X", "Y", "Z"][index] : null).filter(Boolean).join("+")
        : "";
      summary.textContent = `${support.supported ? "" : "desktop processor unavailable · "}${options.displayWindowSeconds}s view · ${scaling}${axes ? ` · ${axes} axes` : ""}`;
      const tune = document.createElement("button");
      tune.type = "button";
      tune.className = "module-tune-button";
      tune.textContent = "Adjust";
      tune.setAttribute("aria-label", `Adjust ${metric.label} module`);
      tune.addEventListener("click", () => openModuleSettings(id));
      controls.append(summary, tune);
      card.append(controls);
      return card;
    }).filter(Boolean);
    for (const formula of app.customFormulas.filter((candidate) => candidate.enabled)) {
      const card = document.createElement("article");
      card.className = "output-card formula-output-card";
      const header = document.createElement("header");
      const identity = document.createElement("span");
      const label = document.createElement("strong");
      label.textContent = formula.name;
      const stream = document.createElement("small");
      stream.textContent = customStreamName(formula, elements["stream-name"].value);
      identity.append(label, stream);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "×";
      remove.setAttribute("aria-label", `Remove ${formula.name}`);
      remove.addEventListener("click", () => {
        app.customFormulas = app.customFormulas.filter((candidate) => candidate.id !== formula.id);
        renderOutputs();
        void configureOutputs();
      });
      header.append(identity, remove);
      const controls = document.createElement("div");
      controls.className = "metric-controls";
      const summary = document.createElement("span");
      summary.className = "module-summary";
      summary.textContent = `${formulaSources[formula.source].label} · ${formula.expression} · ${formula.unit}`;
      const edit = document.createElement("button");
      edit.type = "button";
      edit.className = "module-tune-button";
      edit.textContent = "Edit formula";
      edit.addEventListener("click", () => void openFormulaLab(null, formula.id));
      controls.append(summary, edit);
      card.append(header, controls);
      cards.push(card);
    }
    elements["output-chips"].replaceChildren(...cards);
    const count = app.outputs.size + app.customFormulas.filter((formula) => formula.enabled).length;
    elements["included-count"].textContent = `${count} active`;
    elements["output-state"].textContent = `${count} signal${count === 1 ? "" : "s"}`;
    updateStreamNamePreview();
    rebuildVisualOptions();
    applySourceColor();
  }

  function metricOptionFor(id, { forSelection = false } = {}) {
    const stored = app.metricOptions[id] || {};
    const breathing = breathingOutputIds.has(id)
      ? app.breathingSettings
      : stored.processing?.breathing || stored.processing?.breathingPhase || app.breathingSettings;
    const options = {
      normalization: stored.normalization || (forSelection && id === "acc_breathing_magnitude" ? "slidingWindow" : "none"),
      windowSeconds: Number(stored.windowSeconds) || (id === "acc_breathing_magnitude" ? 20 : 60),
      displayWindowSeconds: Number(stored.displayWindowSeconds) || 5,
      processing: {},
    };
    if (breathingOutputIds.has(id)) {
      options.processing.breathing = {
        axes: Array.isArray(breathing.axes) && breathing.axes.length === 3
          ? breathing.axes.map(Boolean)
          : [true, false, true],
        calibrationWindowSeconds: numberOr(breathing.calibrationWindowSeconds, 12),
        minimumAxisRangeG: numberOr(breathing.minimumAxisRangeG, 0.01),
        smoothingWindowSeconds: numberOr(breathing.smoothingWindowSeconds, 0.75),
        sensitivity: numberOr(breathing.sensitivity, 0.60),
        staleTimeoutSeconds: numberOr(breathing.staleTimeoutSeconds, 3),
        invertDirection: Boolean(breathing.invertDirection),
        adaptiveBounds: breathing.adaptiveBounds !== false,
        adaptiveWindowSeconds: numberOr(breathing.adaptiveWindowSeconds, 20),
        lowerQuantile: numberOr(breathing.lowerQuantile, 0.05),
        upperQuantile: numberOr(breathing.upperQuantile, 0.95),
      };
    }
    return options;
  }

  function numberOr(value, fallback) {
    const numeric = Number(value);
    return Number.isFinite(numeric) ? numeric : fallback;
  }

  function optionIdForVisual(id) {
    return visualDefinitions[id]?.parent || id;
  }

  function openModuleSettings(id) {
    const metric = app.catalog.find((candidate) => candidate.id === id);
    if (!metric || !app.outputs.has(id)) return;
    app.editingModuleId = id;
    app.moduleDraft = structuredClone(metricOptionFor(id));
    elements["module-dialog-title"].textContent = `Adjust ${metric.label}`;
    elements["module-dialog-intro"].textContent = breathingOutputIds.has(id)
      ? "Tune the shared experimental ACC breathing estimate. Saving restarts calibration and applies the settings to both breathing outputs."
      : "Tune this visualizer and output transform. Changes remain a draft until Save module is pressed.";
    renderModuleSettings();
    elements["module-dialog"].showModal();
  }

  function renderModuleSettings() {
    const metric = app.catalog.find((candidate) => candidate.id === app.editingModuleId);
    if (!metric || !app.moduleDraft) return;
    const general = settingsSection("Visualizer and output", "The display window controls only the chart history. Normalization changes both the chart values and values published over LSL or OSC.");
    general.append(numberSetting("Display window", "Seconds visible in the low-latency visualizer.", app.moduleDraft.displayWindowSeconds, 1, 600, 1, (value) => {
      app.moduleDraft.displayWindowSeconds = clampNumber(value, 1, 600, 5);
    }));
    if (metric.normalizable) {
      general.append(selectSetting("Normalization", "Choose original units or a 0–1 transform.", app.moduleDraft.normalization, [
        ["none", "Original units"], ["slidingWindow", "0–1 sliding window"], ["session", "0–1 whole run"],
      ], (value) => {
        app.moduleDraft.normalization = value;
        renderModuleSettings();
      }));
      if (app.moduleDraft.normalization === "slidingWindow") {
        general.append(numberSetting("Normalization window", "Seconds used for running minimum and maximum.", app.moduleDraft.windowSeconds, 5, 3600, 5, (value) => {
          app.moduleDraft.windowSeconds = clampNumber(value, 5, 3600, 60);
        }));
      }
    } else {
      const note = document.createElement("div");
      note.className = "settings-note";
      note.textContent = metric.raw
        ? "Raw device streams stay in native units; only their visualizer history can be adjusted."
        : "This is a categorical or quality signal, so its published class values remain unscaled.";
      general.append(note);
    }

    const sections = [general];
    if (breathingOutputIds.has(metric.id)) {
      const classifier = app.moduleDraft.processing.breathing;
      const processing = settingsSection("Experimental ACC breathing", "X + Z is recommended. Including Y enables the rotational axis; at least two axes are required. Saving restarts the fixed 12-second calibration.");
      ["X axis · recommended", "Y axis · rotational", "Z axis · recommended"].forEach((label, index) => {
        processing.append(checkSetting(label, "Included in smoothing, PCA calibration and projection.", classifier.axes[index], (value) => {
          const next = [...classifier.axes];
          next[index] = value;
          if (selectedAxisCount(next) < 2) {
            toast("Choose at least two axes for the ACC breathing estimate.", true);
            renderModuleSettings();
            return;
          }
          classifier.axes = next;
        }));
      });
      processing.append(
        numberSetting("Smoothing window", "Seconds of ACC smoothing; longer is steadier but adds lag.", classifier.smoothingWindowSeconds, 0.05, 5, 0.05, (value) => classifier.smoothingWindowSeconds = clampNumber(value, 0.05, 5, 0.75)),
        checkSetting("Invert direction", "Flips the learned movement axis when strap orientation reverses the curve.", classifier.invertDirection, (value) => classifier.invertDirection = value),
      );
      if (metric.id === "breathing_phase") {
        processing.append(numberSetting("Sensitivity", "0 is conservative; 1 reacts to smaller projected changes.", classifier.sensitivity, 0, 1, 0.05, (value) => classifier.sensitivity = clampNumber(value, 0, 1, 0.60)));
      }
      processing.append(
        numberSetting("Calibration window", "Quiet seconds used to learn the principal motion axis.", classifier.calibrationWindowSeconds, 1, 60, 1, (value) => classifier.calibrationWindowSeconds = clampNumber(value, 1, 60, 12)),
        numberSetting("Minimum axis range", "Minimum selected-axis calibration travel in g.", classifier.minimumAxisRangeG, 0.001, 0.25, 0.001, (value) => classifier.minimumAxisRangeG = clampNumber(value, 0.001, 0.25, 0.01)),
        numberSetting("Stale timeout", "Notification gap in seconds that forces not-ready / pause output.", classifier.staleTimeoutSeconds, 0.25, 30, 0.25, (value) => classifier.staleTimeoutSeconds = clampNumber(value, 0.25, 30, 3)),
        checkSetting("Adaptive bounds", "Update accepted calibration quantiles from recent projected motion.", classifier.adaptiveBounds, (value) => {
          classifier.adaptiveBounds = value;
          renderModuleSettings();
        }),
      );
      if (classifier.adaptiveBounds) {
        processing.append(
          numberSetting("Adaptive window", "Seconds retained for recent projection quantiles.", classifier.adaptiveWindowSeconds, 5, 300, 1, (value) => classifier.adaptiveWindowSeconds = clampNumber(value, 5, 300, 20)),
          numberSetting("Lower quantile", "Low robust bound; allowed range 0.00–0.40.", classifier.lowerQuantile, 0, 0.40, 0.01, (value) => classifier.lowerQuantile = clampNumber(value, 0, 0.40, 0.05)),
          numberSetting("Upper quantile", "High robust bound; allowed range 0.60–1.00.", classifier.upperQuantile, 0.60, 1, 0.01, (value) => classifier.upperQuantile = clampNumber(value, 0.60, 1, 0.95)),
        );
      }
      const warning = document.createElement("div");
      warning.className = "settings-note";
      warning.textContent = "Unvalidated research estimate. Pause (0) also represents calibration or stale input; it does not certify good signal. Inspect raw ACC and compare with a reference respiratory sensor.";
      processing.append(warning);
      sections.push(processing);
    }
    elements["module-settings"].replaceChildren(...sections);
    elements["module-dialog-status"].textContent = "Draft settings · press Save module to apply";
  }

  function settingsSection(titleText, descriptionText) {
    const section = document.createElement("section");
    section.className = "settings-section";
    const title = document.createElement("h3");
    title.textContent = titleText;
    const description = document.createElement("p");
    description.textContent = descriptionText;
    section.append(title, description);
    return section;
  }

  function numberSetting(labelText, helpText, value, min, max, step, onInput) {
    const field = settingField(labelText, helpText);
    const input = document.createElement("input");
    input.type = "number";
    input.min = String(min);
    input.max = String(max);
    input.step = String(step);
    input.value = String(value);
    input.addEventListener("input", () => onInput(input.value));
    field.insertBefore(input, field.lastChild);
    return field;
  }

  function selectSetting(labelText, helpText, value, choices, onChange) {
    const field = settingField(labelText, helpText);
    const select = document.createElement("select");
    for (const [choiceValue, choiceText] of choices) {
      const option = document.createElement("option");
      option.value = choiceValue;
      option.textContent = choiceText;
      select.append(option);
    }
    select.value = value;
    select.addEventListener("change", () => onChange(select.value));
    field.insertBefore(select, field.lastChild);
    return field;
  }

  function checkSetting(labelText, helpText, value, onChange) {
    const field = settingField(labelText, helpText);
    field.classList.add("setting-check");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = value;
    input.addEventListener("change", () => onChange(input.checked));
    field.prepend(input);
    return field;
  }

  function settingField(labelText, helpText) {
    const field = document.createElement("label");
    field.className = "setting-field";
    const label = document.createElement("span");
    label.textContent = labelText;
    const help = document.createElement("small");
    help.textContent = helpText;
    field.append(label, help);
    return field;
  }

  function clampNumber(value, minimum, maximum, fallback) {
    const number = Number(value);
    return Math.max(minimum, Math.min(maximum, Number.isFinite(number) ? number : fallback));
  }

  async function saveModuleSettings() {
    const id = app.editingModuleId;
    const metric = app.catalog.find((candidate) => candidate.id === id);
    if (!id || !metric || !app.moduleDraft) return;
    const draft = structuredClone(app.moduleDraft);
    if (!metric.normalizable) draft.normalization = "none";
    if (breathingOutputIds.has(id)) {
      if (breathingDraftIsInvalid(draft.processing.breathing)) {
        toast("Choose at least two axes and keep 0.10 between calibration quantiles.", true);
        return;
      }
      app.breathingSettings = structuredClone(draft.processing.breathing);
    }
    app.metricOptions[id] = draft;
    resetVisualTransform(id);
    for (const [visualId, definition] of Object.entries(visualDefinitions)) {
      if (definition.parent === id) resetVisualTransform(visualId);
    }
    renderOutputs();
    updateVisualLabels();
    await configureOutputs();
    elements["module-dialog"].close();
    toast(`${metric.label} settings saved${breathingOutputIds.has(id) ? " · calibration restarted" : ""}`);
  }

  function resetVisualTransform(id) {
    delete app.visualNormalizers[id];
    ensureBuffer(id).clear();
  }

  function resetMeasurementVisuals() {
    app.visualNormalizers = {};
    for (const buffer of Object.values(buffers)) buffer.clear();
    resetPhaseMotion();
    app.sampleCount = 0;
    markTelemetryDirty();
  }

  function visualValue(id, input) {
    const value = Number(input);
    if (!Number.isFinite(value)) return value;
    const options = metricOptionFor(optionIdForVisual(id));
    if (!options || options.normalization === "none") return value;
    const modeKey = `${options.normalization}:${options.windowSeconds || 60}`;
    let state = app.visualNormalizers[id];
    if (!state || state.modeKey !== modeKey) {
      state = { modeKey, minimum: Infinity, maximum: -Infinity, sequence: 0, minQueue: [], maxQueue: [], minHead: 0, maxHead: 0 };
      app.visualNormalizers[id] = state;
    }
    if (options.normalization === "session") {
      state.minimum = Math.min(state.minimum, value);
      state.maximum = Math.max(state.maximum, value);
      return minMaxValue(value, state.minimum, state.maximum);
    }
    const now = performance.now();
    state.sequence += 1;
    while (state.minQueue.length > state.minHead && state.minQueue.at(-1).value >= value) state.minQueue.pop();
    while (state.maxQueue.length > state.maxHead && state.maxQueue.at(-1).value <= value) state.maxQueue.pop();
    state.minQueue.push({ at: now, value });
    state.maxQueue.push({ at: now, value });
    const cutoff = now - Math.max(5, Number(options.windowSeconds) || 60) * 1000;
    while (state.minQueue[state.minHead]?.at < cutoff) state.minHead += 1;
    while (state.maxQueue[state.maxHead]?.at < cutoff) state.maxHead += 1;
    if (state.minHead > 1024) { state.minQueue = state.minQueue.slice(state.minHead); state.minHead = 0; }
    if (state.maxHead > 1024) { state.maxQueue = state.maxQueue.slice(state.maxHead); state.maxHead = 0; }
    return minMaxValue(value, state.minQueue[state.minHead]?.value ?? value, state.maxQueue[state.maxHead]?.value ?? value);
  }

  function minMaxValue(value, minimum, maximum) {
    return Math.abs(maximum - minimum) < Number.EPSILON ? 0.5 : Math.max(0, Math.min(1, (value - minimum) / (maximum - minimum)));
  }

  function updateStreamNamePreview() {
    const byId = new Map(app.catalog.map((metric) => [metric.id, metric]));
    const names = [...app.outputs]
      .map((id) => byId.get(id))
      .filter(Boolean)
      .map((metric) => streamOutputName(metric, elements["stream-name"].value));
    names.push(...app.customFormulas.filter((formula) => formula.enabled).map((formula) => customStreamName(formula, elements["stream-name"].value)));
    if (!normalizeStreamBase(elements["stream-name"].value)) {
      elements["stream-name-preview"].textContent = "Use at least one letter or number; spaces become underscores.";
      return;
    }
    const extra = names.length > 2 ? ` · +${names.length - 2} more` : "";
    const prefix = runtime.isBrowser ? "Available in this tab" : "Publishes";
    elements["stream-name-preview"].textContent = names.length
      ? `${prefix} ${names.slice(0, 2).join(" · ")}${extra}`
      : "No outputs are currently selected.";
  }

  function rebuildVisualOptions() {
    const choices = [];
    for (const [id, definition] of Object.entries(visualDefinitions)) {
      const parent = definition.parent || id;
      const enabled = definition.formulaId
        ? app.customFormulas.some((formula) => formula.id === definition.formulaId && formula.enabled)
        : app.outputs.has(parent);
      if (!enabled) continue;
      choices.push({ id, definition });
    }
    elements["visual-source"].replaceChildren(...choices.map(({ id, definition }) => {
      const option = document.createElement("option");
      option.value = id;
      option.textContent = definition.label;
      return option;
    }));
    if (!choices.some((choice) => choice.id === app.selectedVisual)) {
      app.selectedVisual = choices[0]?.id || "";
    }
    elements["visual-source"].value = app.selectedVisual;
    elements["visual-source"].disabled = !choices.length;
    updateVisualLabels();
  }

  function updateVisualLabels() {
    const definition = visualDefinitions[app.selectedVisual];
    const breathingTrail = app.selectedVisual === "breathing_volume";
    const custom = Boolean(definition?.formulaId);
    const options = custom
      ? { normalization: "none", windowSeconds: 60, displayWindowSeconds: 5 }
      : metricOptionFor(optionIdForVisual(app.selectedVisual));
    const normalized = options.normalization !== "none";
    elements["visual-unit"].textContent = app.selectedVisual === "breathing_phase" ? "" : normalized ? "0–1" : definition?.unit || "";
    elements["visual-window-label"].textContent = app.selectedVisual === "breathing_phase" ? "Live phase" : `${options.displayWindowSeconds} second window`;
    elements["visual-scale-label"].textContent = breathingTrail
      ? "Relative 0–1 waveform"
      : normalized
        ? options.normalization === "session" ? "0–1 whole run" : `0–1 / ${options.windowSeconds}s`
        : "Original scale";
    elements["adjust-visual"].disabled = !definition || custom;
    elements["chart-shell"].classList.toggle("phase-visual", app.selectedVisual === "breathing_phase");
    elements["chart-shell"].classList.toggle("breathing-trail-visual", breathingTrail);
    elements["chart-shell"].classList.toggle("stacked-axes", Boolean(definition?.channels));
    elements["visual-current"].classList.toggle("stacked-value", Boolean(definition?.channels));
    elements["signal-canvas"].setAttribute(
      "aria-label",
      breathingTrail
        ? "Preliminary one-dimensional ACC breathing waveform. The newest sample is a moving dot and recent samples form a leftward trail; rising follows the configured inhale direction."
        : definition?.channels
          ? "Live raw accelerometer X, Y, and Z signals in three stacked plots"
          : `Live ${definition?.label || "selected Polar H10"} signal`,
    );
    if (!breathingTrail) {
      delete elements["signal-canvas"].dataset.visualMode;
      delete elements["signal-canvas"].dataset.breathDirection;
      delete elements["signal-canvas"].dataset.latestY01;
      delete elements["signal-canvas"].dataset.trailPoints;
    }
    const legendItems = definition?.channels || (definition ? [{
      label: breathingTrail ? `${definition.label} · dot = latest` : definition.label,
      color: selectedSourceColor(definition.color),
    }] : []);
    elements["visual-legend"].replaceChildren(...legendItems.map((item) => {
      const legend = document.createElement("span");
      legend.className = "legend-item";
      const line = document.createElement("i");
      line.className = "legend-line";
      line.style.background = item.color || "#87958d";
      const label = document.createElement("strong");
      label.textContent = item.label;
      legend.append(line, label);
      return legend;
    }));
    requestRender();
  }

  function handleNativeDestinationToggle(protocol) {
    const normalized = protocol.toLowerCase();
    const toggle = elements[`${normalized}-toggle`];
    if (runtime.isBrowser) {
      toggle.checked = false;
      elements["native-output-browser-error-text"].textContent = `${protocol} output is supported only by the installed Polar Stream app.`;
      elements["native-output-browser-error"].hidden = false;
      addActivity(`${protocol} output requires the installed app`);
      toast(`${protocol} is available only in the installed Polar Stream app. Use the download link below.`, true);
      return;
    }
    void configureOutputs();
  }

  async function configureOutputs({ quiet = false } = {}) {
    const streamName = normalizeStreamBase(elements["stream-name"].value);
    if (!streamName) {
      elements["stream-name"].setAttribute("aria-invalid", "true");
      updateStreamNamePreview();
      return;
    }
    elements["stream-name"].removeAttribute("aria-invalid");
    app.streamName = streamName;
    const config = {
      streamName,
      lslEnabled: runtime.isBrowser ? false : elements["lsl-toggle"].checked,
      oscEnabled: runtime.isBrowser ? false : elements["osc-toggle"].checked,
      csvEnabled: elements["csv-toggle"].checked,
      audioEnabled: elements["audio-toggle"].checked,
      outputs: [...app.outputs],
      metricOptions: Object.fromEntries([...app.outputs].map((id) => [id, metricOptionFor(id)])),
      customFormulas: app.customFormulas.map((formula) => ({ ...formula })),
    };
    const sequence = ++app.outputSequence;
    try {
      const health = await runtime.updateOutputConfig(config);
      if (sequence !== app.outputSequence) return;
      if (runtime.isBrowser && browserSession) {
        browserSession.configure({
          ...config,
          metricUnits: Object.fromEntries(app.catalog.map((metric) => [metric.id, metric.unit])),
        });
      }
      audioDataLink?.configure(config);
      app.streamName = health.streamName || streamName;
      elements["stream-name"].value = app.streamName;
      config.streamName = app.streamName;
      app.preferences = !isNative
        ? preferences.saveOutputConfig(config)
        : { ...app.preferences, streamName: config.streamName, outputConfig: structuredClone(config) };
      renderOutputs();
      updateDestinationHealth(health);
    } catch (error) {
      if (runtime.isBrowser) {
        elements["lsl-toggle"].checked = false;
        elements["osc-toggle"].checked = false;
      }
      if (!quiet) toast(runtime.formatError(error), true);
    }
  }

  function updateDestinationHealth(health) {
    const lslText = runtime.isBrowser
      ? health.lsl
      : elements["lsl-toggle"].checked ? health.lsl : "Local network · time synchronized";
    const oscText = runtime.isBrowser
      ? health.osc
      : elements["osc-toggle"].checked ? health.osc : "UDP · localhost:9000";
    const csvText = runtime.isBrowser
      ? elements["csv-detail"].textContent
      : elements["csv-toggle"].checked ? health.csv : "All received raw data and produced metrics · bounded writer";
    const audioText = elements["audio-toggle"].checked
      ? health.audio || audioDataLink?.supportStatus().reason
      : "CRC-checked stereo data modem · cable or digital recording";
    elements["lsl-detail"].textContent = lslText;
    elements["osc-detail"].textContent = oscText;
    if (!runtime.isBrowser) elements["csv-detail"].textContent = csvText;
    if (!audioDataLink?.status().enabled) elements["audio-detail"].textContent = audioText;
    elements["lsl-detail"].classList.toggle("warning", runtime.isBrowser || (elements["lsl-toggle"].checked && /not found|failed|could not|unavailable/i.test(lslText)));
    elements["osc-detail"].classList.toggle("warning", runtime.isBrowser || (elements["osc-toggle"].checked && /failed|could not|unavailable/i.test(oscText)));
    elements["csv-detail"].classList.toggle("warning", elements["csv-toggle"].checked && /stopped|failed|could not|unavailable/i.test(csvText));
  }

  function resizeCanvas() {
    const canvas = elements["signal-canvas"];
    const bounds = elements["chart-shell"].getBoundingClientRect();
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.max(1, Math.round(bounds.width * ratio));
    canvas.height = Math.max(1, Math.round(bounds.height * ratio));
  }

  const renderIntervalMs = 1000 / 30;
  const telemetryIntervalMs = 100;
  let renderFrameId = 0;
  let renderTimerId = 0;
  let renderPending = false;
  let lastRenderAt = 0;
  let lastTelemetryAt = 0;
  let telemetryDirty = true;
  let previousFrame = 0;
  let frameAccumulator = 0;
  let frameSamples = 0;

  function scheduleRender(delayMs = 0) {
    if (isInterfaceRenderer || document.hidden || renderPending) return;
    renderPending = true;
    const begin = () => {
      renderTimerId = 0;
      renderFrameId = window.requestAnimationFrame(drawFrame);
    };
    if (delayMs > 1) renderTimerId = window.setTimeout(begin, delayMs);
    else begin();
  }

  function requestRender() {
    const elapsed = performance.now() - lastRenderAt;
    scheduleRender(lastRenderAt ? Math.max(0, renderIntervalMs - elapsed) : 0);
  }

  function cancelScheduledRender() {
    if (renderTimerId) window.clearTimeout(renderTimerId);
    if (renderFrameId) window.cancelAnimationFrame(renderFrameId);
    renderTimerId = 0;
    renderFrameId = 0;
    renderPending = false;
  }

  function drawFrame(now) {
    renderPending = false;
    renderFrameId = 0;
    lastRenderAt = now;
    const elapsed = previousFrame ? now - previousFrame : 0;
    previousFrame = now;
    if (elapsed > 0 && elapsed < 250) {
      frameAccumulator += elapsed;
      frameSamples += 1;
    } else if (elapsed >= 250) {
      frameAccumulator = 0;
      frameSamples = 0;
    }
    if (frameAccumulator >= 800) {
      elements["render-rate"].textContent = `${Math.round((frameSamples * 1000) / frameAccumulator)} fps`;
      frameAccumulator = 0;
      frameSamples = 0;
    }

    if (telemetryDirty && now - lastTelemetryAt >= telemetryIntervalMs) {
      refreshTelemetry();
      telemetryDirty = false;
      lastTelemetryAt = now;
    }
    drawSignal();
    if (telemetryDirty) {
      scheduleRender(Math.max(renderIntervalMs, telemetryIntervalMs - (now - lastTelemetryAt)));
    }
  }

  function drawSignal() {
    const canvas = elements["signal-canvas"];
    const context = signalContext;
    const definition = visualDefinitions[app.selectedVisual];
    if (!definition) {
      context.clearRect(0, 0, canvas.width, canvas.height);
      elements["chart-empty"].hidden = false;
      elements["visual-current"].textContent = "—";
      return;
    }

    const options = metricOptionFor(optionIdForVisual(app.selectedVisual));
    if (definition.channels) {
      drawStackedSignal(context, canvas, definition, options);
      return;
    }

    const buffer = buffers[app.selectedVisual];
    if (!buffer) return;
    if (app.selectedVisual === "breathing_phase") {
      drawBreathingPhase(context, canvas, buffer);
      return;
    }
    if (app.selectedVisual === "breathing_volume") {
      drawBreathingTrail(context, canvas, buffer, definition, options);
      return;
    }

    const visibleCount = Math.max(10, Math.ceil(definition.rate * options.displayWindowSeconds));
    const valueCount = buffer.tailSize(visibleCount);
    const normalized = options.normalization !== "none";
    elements["chart-empty"].hidden = valueCount > 1;
    elements["visual-current"].textContent = formatValue(buffer.latest(), normalized ? 3 : definition.unit === "g" ? 3 : definition.unit === "bpm" ? 0 : 1);
    if (valueCount < 2) {
      context.clearRect(0, 0, canvas.width, canvas.height);
      return;
    }

    let min = Infinity;
    let max = -Infinity;
    for (let index = 0; index < valueCount; index += 1) {
      const value = buffer.tailValue(index, valueCount);
      if (value < min) min = value;
      if (value > max) max = value;
    }
    if (normalized) {
      min = 0;
      max = 1;
    } else if (definition.symmetric) {
      const extent = Math.max(Math.abs(min), Math.abs(max), 1) * 1.08;
      min = -extent;
      max = extent;
    } else {
      const padding = Math.max((max - min) * 0.12, Math.abs(max) * 0.02, 0.5);
      min -= padding;
      max += padding;
    }

    elements["y-max"].textContent = shortAxis(max);
    elements["y-min"].textContent = shortAxis(min);
    const width = canvas.width;
    const height = canvas.height;
    const padX = Math.round(width * 0.035);
    const padY = Math.round(height * 0.08);
    const drawWidth = width - padX * 2;
    const drawHeight = height - padY * 2;
    const range = max - min || 1;
    context.clearRect(0, 0, width, height);
    context.beginPath();
    for (let index = 0; index < valueCount; index += 1) {
      const value = buffer.tailValue(index, valueCount);
      const x = padX + (index / (valueCount - 1)) * drawWidth;
      const y = padY + (1 - (value - min) / range) * drawHeight;
      if (index === 0) context.moveTo(x, y); else context.lineTo(x, y);
    }
    context.strokeStyle = selectedSourceColor(definition.color);
    context.lineWidth = Math.max(1.4, (window.devicePixelRatio || 1) * 0.9);
    context.lineJoin = "round";
    context.lineCap = "round";
    context.stroke();
  }

  function colorWithAlpha(color, alpha) {
    const match = /^#([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i.exec(color);
    if (!match) return color;
    return `rgba(${Number.parseInt(match[1], 16)}, ${Number.parseInt(match[2], 16)}, ${Number.parseInt(match[3], 16)}, ${alpha})`;
  }

  function drawBreathingTrail(context, canvas, buffer, definition, options) {
    const visibleCount = Math.max(10, Math.ceil(definition.rate * options.displayWindowSeconds));
    const valueCount = buffer.tailSize(visibleCount);
    const latest = Number(buffer.latest());
    const hasData = valueCount > 1 && Number.isFinite(latest);
    elements["chart-empty"].hidden = hasData;
    elements["visual-current"].textContent = Number.isFinite(latest) ? formatValue(latest, 3) : "—";
    elements["y-max"].textContent = "";
    elements["y-min"].textContent = "";

    const width = canvas.width;
    const height = canvas.height;
    context.clearRect(0, 0, width, height);
    canvas.dataset.visualMode = "breathing-trail";
    canvas.dataset.trailPoints = String(valueCount);
    if (!hasData) {
      delete canvas.dataset.breathDirection;
      delete canvas.dataset.latestY01;
      return;
    }

    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    const padLeft = Math.max(Math.round(width * 0.06), Math.round(54 * pixelRatio));
    const padRight = Math.max(Math.round(width * 0.04), Math.round(30 * pixelRatio));
    const padY = Math.max(Math.round(height * 0.10), Math.round(24 * pixelRatio));
    const drawWidth = width - padLeft - padRight;
    const drawHeight = height - padY * 2;
    const sourceColor = selectedSourceColor(definition.color);
    const point = (index) => {
      const value = Math.max(0, Math.min(1, Number(buffer.tailValue(index, valueCount)) || 0));
      return {
        x: padLeft + (index / (valueCount - 1)) * drawWidth,
        y: padY + (1 - value) * drawHeight,
        value,
      };
    };
    const latestPoint = point(valueCount - 1);
    const trendLookback = Math.min(5, valueCount - 1);
    const trend = latestPoint.value - point(valueCount - 1 - trendLookback).value;
    const direction = trend > 0.002 ? "inhale" : trend < -0.002 ? "exhale" : "pause";
    canvas.dataset.breathDirection = direction;
    canvas.dataset.latestY01 = latestPoint.value.toFixed(4);

    context.save();
    context.lineCap = "round";
    context.lineJoin = "round";
    context.textBaseline = "middle";
    context.font = `700 ${Math.round(9 * pixelRatio)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
    context.fillStyle = "#168259";
    context.fillText("INHALE ↑", Math.round(11 * pixelRatio), padY);
    context.fillStyle = "#b86520";
    context.fillText("EXHALE ↓", Math.round(11 * pixelRatio), height - padY);

    context.beginPath();
    context.moveTo(padLeft, padY + drawHeight / 2);
    context.lineTo(width - padRight, padY + drawHeight / 2);
    context.strokeStyle = "rgba(81, 103, 91, 0.22)";
    context.lineWidth = Math.max(1, pixelRatio * 0.7);
    context.setLineDash([4 * pixelRatio, 7 * pixelRatio]);
    context.stroke();
    context.setLineDash([]);

    context.beginPath();
    for (let index = 0; index < valueCount; index += 1) {
      const current = point(index);
      if (index === 0) context.moveTo(current.x, current.y); else context.lineTo(current.x, current.y);
    }
    const trailGradient = context.createLinearGradient(padLeft, 0, width - padRight, 0);
    trailGradient.addColorStop(0, colorWithAlpha(sourceColor, 0.10));
    trailGradient.addColorStop(0.32, colorWithAlpha(sourceColor, 0.40));
    trailGradient.addColorStop(1, sourceColor);
    context.strokeStyle = trailGradient;
    context.lineWidth = Math.max(2, pixelRatio * 1.5);
    context.stroke();

    const glow = context.createRadialGradient(
      latestPoint.x, latestPoint.y, 0,
      latestPoint.x, latestPoint.y, 13 * pixelRatio,
    );
    glow.addColorStop(0, colorWithAlpha(sourceColor, 0.55));
    glow.addColorStop(1, colorWithAlpha(sourceColor, 0));
    context.beginPath();
    context.arc(latestPoint.x, latestPoint.y, 13 * pixelRatio, 0, Math.PI * 2);
    context.fillStyle = glow;
    context.fill();
    context.beginPath();
    context.arc(latestPoint.x, latestPoint.y, 5.5 * pixelRatio, 0, Math.PI * 2);
    context.fillStyle = "#fbfcfa";
    context.fill();
    context.strokeStyle = sourceColor;
    context.lineWidth = Math.max(2, pixelRatio * 1.7);
    context.stroke();

    const descriptor = direction === "inhale" ? "INHALE ↑" : direction === "exhale" ? "EXHALE ↓" : "PAUSE";
    context.textAlign = "right";
    context.font = `800 ${Math.round(10 * pixelRatio)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
    context.fillStyle = direction === "inhale" ? "#168259" : direction === "exhale" ? "#b86520" : "#3b78aa";
    context.fillText(descriptor, width - Math.round(11 * pixelRatio), Math.round(14 * pixelRatio));
    context.restore();
  }

  function drawStackedSignal(context, canvas, definition, options) {
    const visibleCount = Math.max(10, Math.ceil(definition.rate * options.displayWindowSeconds));
    const series = definition.channels.map((channel) => ({
      ...channel,
      buffer: buffers[channel.buffer],
      valueCount: buffers[channel.buffer].tailSize(visibleCount),
    }));
    const hasData = series.some(({ valueCount }) => valueCount > 1);
    elements["chart-empty"].hidden = hasData;
    elements["visual-current"].textContent = series
      .map(({ label, buffer }) => `${label} ${formatValue(buffer.latest(), 0)}`)
      .join("  ·  ");
    elements["y-max"].textContent = "—";
    elements["y-min"].textContent = "—";

    const width = canvas.width;
    const height = canvas.height;
    context.clearRect(0, 0, width, height);
    if (!hasData) return;

    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    const padLeft = Math.max(Math.round(width * 0.08), Math.round(52 * pixelRatio));
    const padRight = Math.round(width * 0.035);
    const padY = Math.round(height * 0.035);
    const laneGap = Math.round(10 * pixelRatio);
    const laneHeight = (height - padY * 2 - laneGap * (series.length - 1)) / series.length;
    const drawWidth = width - padLeft - padRight;
    context.textBaseline = "middle";
    context.lineJoin = "round";
    context.lineCap = "round";

    series.forEach(({ label, color, symmetric, buffer, valueCount }, seriesIndex) => {
      const laneTop = padY + seriesIndex * (laneHeight + laneGap);
      const laneBottom = laneTop + laneHeight;
      if (seriesIndex > 0) {
        const separatorY = laneTop - laneGap / 2;
        context.beginPath();
        context.moveTo(padLeft, separatorY);
        context.lineTo(width - padRight, separatorY);
        context.strokeStyle = "rgba(81, 103, 91, 0.18)";
        context.lineWidth = Math.max(1, pixelRatio * 0.6);
        context.stroke();
      }

      context.fillStyle = color;
      context.font = `700 ${Math.round(10 * pixelRatio)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
      context.fillText(label, Math.round(10 * pixelRatio), laneTop + laneHeight / 2);
      if (valueCount < 2) return;

      let min = Infinity;
      let max = -Infinity;
      for (let index = 0; index < valueCount; index += 1) {
        const value = buffer.tailValue(index, valueCount);
        if (value < min) min = value;
        if (value > max) max = value;
      }
      if (symmetric) {
        const extent = Math.max(Math.abs(min), Math.abs(max), 1) * 1.08;
        min = -extent;
        max = extent;
      } else {
        const padding = Math.max((max - min) * 0.12, Math.abs(max) * 0.02, 0.5);
        min -= padding;
        max += padding;
      }

      context.fillStyle = "#85928a";
      context.font = `${Math.round(8 * pixelRatio)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
      context.textAlign = "right";
      context.fillText(shortAxis(max), padLeft - Math.round(6 * pixelRatio), laneTop + Math.round(7 * pixelRatio));
      context.fillText("0", padLeft - Math.round(6 * pixelRatio), laneTop + laneHeight / 2);
      context.fillText(shortAxis(min), padLeft - Math.round(6 * pixelRatio), laneBottom - Math.round(7 * pixelRatio));
      context.textAlign = "left";

      const range = max - min || 1;
      context.beginPath();
      for (let index = 0; index < valueCount; index += 1) {
        const x = padLeft + (index / (valueCount - 1)) * drawWidth;
        const y = laneTop + (1 - (buffer.tailValue(index, valueCount) - min) / range) * laneHeight;
        if (index === 0) context.moveTo(x, y); else context.lineTo(x, y);
      }
      context.strokeStyle = color;
      context.lineWidth = Math.max(1.4, pixelRatio * 0.9);
      context.stroke();
    });
  }

  function resetPhaseMotion(level = 0.5, velocity = 0) {
    app.phaseMotion = { level, velocity, lastAt: 0 };
  }

  function advancePhaseMotion(phase, now = performance.now()) {
    const state = app.phaseMotion;
    const deltaSeconds = state.lastAt
      ? Math.max(1 / 240, Math.min(0.08, (now - state.lastAt) / 1000))
      : 1 / 30;
    state.lastAt = now;
    const targetVelocity = phase > 0.5 ? 0.95 : phase < -0.5 && phase > -1.5 ? -0.95 : 0;
    const response = targetVelocity === 0 ? 1.7 : 4.2;
    state.velocity += (targetVelocity - state.velocity) * (1 - Math.exp(-response * deltaSeconds));
    if (state.velocity >= 0) {
      state.level += state.velocity * deltaSeconds * (1 - state.level) * 1.65;
    } else {
      state.level += state.velocity * deltaSeconds * state.level * 1.65;
    }
    state.level = Math.max(0, Math.min(1, state.level));
    return state.level;
  }

  function drawBreathingPhase(context, canvas, phaseBuffer, now = performance.now()) {
    const phase = phaseBuffer.latest();
    const width = canvas.width;
    const height = canvas.height;
    context.clearRect(0, 0, width, height);
    const descriptor = breathingPhaseDescriptor(phase);
    const hasPhase = phase != null;
    elements["chart-empty"].hidden = hasPhase;
    elements["visual-current"].textContent = hasPhase ? descriptor.label : "—";
    elements["y-max"].textContent = "";
    elements["y-min"].textContent = "";
    if (!hasPhase) return;

    const motion = advancePhaseMotion(Number(phase), now);
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    const shortestSide = Math.min(width, height);
    const minimumRadius = shortestSide * 0.14;
    const maximumRadius = shortestSide * 0.34;
    const radius = minimumRadius + motion * (maximumRadius - minimumRadius);
    const centerX = width / 2;
    const centerY = height / 2;
    const glow = context.createRadialGradient(centerX, centerY, radius * 0.25, centerX, centerY, radius * 1.35);
    glow.addColorStop(0, `${descriptor.color}d9`);
    glow.addColorStop(0.68, `${descriptor.color}8c`);
    glow.addColorStop(1, `${descriptor.color}00`);
    context.beginPath();
    context.arc(centerX, centerY, radius * 1.35, 0, Math.PI * 2);
    context.fillStyle = glow;
    context.fill();
    context.beginPath();
    context.arc(centerX, centerY, radius, 0, Math.PI * 2);
    context.fillStyle = descriptor.textColor;
    context.globalAlpha = 0.22;
    context.fill();
    context.globalAlpha = 1;
    context.strokeStyle = descriptor.color;
    context.lineWidth = Math.max(2, pixelRatio * 2);
    context.stroke();
    context.fillStyle = descriptor.color;
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.font = `800 ${Math.max(13, Math.min(width, height) * 0.045)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
    context.fillText(descriptor.label, centerX, centerY);
    context.font = `600 ${Math.max(9, Math.min(width, height) * 0.026)}px system-ui, sans-serif`;
    context.globalAlpha = 0.72;
    context.fillText(descriptor.hint, centerX, centerY + Math.max(22, radius * 0.27));
    context.globalAlpha = 1;
  }

  function breathingPhaseDescriptor(value) {
    if (value > 0.5) return { label: "INHALE", hint: "circle expanding", color: "#168259", textColor: "#083d29" };
    if (value < -0.5 && value > -1.5) return { label: "EXHALE", hint: "circle shrinking", color: "#d17a28", textColor: "#673605" };
    return { label: "PAUSE", hint: "motion easing to rest", color: "#3b78aa", textColor: "#173f62" };
  }

  function updateSparkline() {
    const buffer = buffers.raw_ecg;
    const valueCount = buffer.tailSize(56);
    if (valueCount < 2) return;
    let min = Infinity;
    let max = -Infinity;
    for (let index = 0; index < valueCount; index += 1) {
      const value = buffer.tailValue(index, valueCount);
      min = Math.min(min, value);
      max = Math.max(max, value);
    }
    const range = max - min || 1;
    const path = [];
    for (let index = 0; index < valueCount; index += 1) {
      const x = (index / (valueCount - 1)) * 280;
      const y = 4 + (1 - (buffer.tailValue(index, valueCount) - min) / range) * 36;
      path.push(`${index ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`);
    }
    elements["ecg-spark"].setAttribute("d", path.join(" "));
  }

  function formatValue(value, digits = 1) {
    const number = Number(value);
    if (!Number.isFinite(number)) return "—";
    return number.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
  }

  function shortAxis(value) {
    const absolute = Math.abs(value);
    if (absolute >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
    if (absolute < 10) return value.toFixed(1);
    return Math.round(value).toString();
  }

  async function renderInterfaceScenario(name) {
    if (name === "multiple-colored-sources") {
      const polarSource = { id: "source-1", slot: "source-1", label: "Source 1", color: "#00c2ff", inputKind: "polarH10" };
      const vernierSource = { id: "source-2", slot: "source-2", label: "Source 2", color: "#ffb000", inputKind: "vernierGoDirect" };
      handleNativeEvent({
        kind: "connection", source: polarSource, connected: true, streaming: true,
        deviceName: "Polar H10 A", batteryPercent: 88, message: "Raw ECG and accelerometer are streaming",
      });
      handleNativeEvent({
        kind: "connection", source: vernierSource, connected: true, streaming: true,
        deviceName: "GDX-RB A", batteryPercent: null, deviceModel: "GDX-RB",
        sensorNumber: 1, sensorName: "Force", sensorUnit: "N", samplePeriodUs: 100000,
        message: "Verified Force (N) on channel 1 is streaming at 10.0 Hz",
      });
      handleNativeEvent({
        kind: "force", source: vernierSource, sensorNumber: 1, samplePeriodUs: 100000,
        values: Array.from({ length: 80 }, (_, index) => 3.2 + Math.sin(index / 7) * 0.45),
      });
      refreshTelemetry();
      app.outputs.add("raw_force");
      renderOutputs();
      app.selectedVisual = "raw_force";
      rebuildVisualOptions();
      elements["visual-source"].value = "raw_force";
      updateVisualLabels();
      resizeCanvas();
      drawSignal();
      await new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));
      return {
        scenario: name,
        selectedSource: app.selectedSourceId,
        sourceOptions: [...elements["visual-device"].options].map((option) => option.value),
        chipColors: [...elements["active-source-strip"].children].map((chip) => chip.style.getPropertyValue("--source-color")),
        chartColor: elements["chart-shell"].style.getPropertyValue("--source-color"),
        outputColors: [...elements["output-chips"].children].map((card) => card.style.getPropertyValue("--source-color")),
        forceValue: elements["raw-force-value"].textContent,
        streamName: streamOutputName(app.catalog.find((metric) => metric.id === "raw_force")),
      };
    }
    if (name === "metric-library-previews") {
      await Promise.all([ensureMetricPreviews(), ensurePreviewRecording()]);
      app.selectedMetricId = "raw_ecg";
      app.libraryMetricDraft = structuredClone(metricOptionFor("raw_ecg", { forSelection: true }));
      app.metricFamily = "ecg";
      app.metricFilter = "All";
      app.metricSearch = "";
      elements["metric-search"].value = "";
      updateMetricFamilyUi();
      renderMetricFilters();
      renderMetricOptions();
      if (!elements["output-dialog"].open) elements["output-dialog"].showModal();
      await new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));
      const previewIds = Object.keys(metricPreviews?.metrics || {});
      return {
        scenario: name,
        dialogOpen: elements["output-dialog"].open,
        catalogCount: app.catalog.length,
        visibleCount: libraryCatalog().length,
        visibleIds: libraryCatalog().map((metric) => metric.id),
        previewCount: previewIds.length,
        missingPreviewIds: app.catalog.map((metric) => metric.id).filter((id) => !previewIds.includes(id)),
        source: structuredClone(metricPreviews?.source || {}),
        detailCoverage: app.catalog.map((metric) => ({
          id: metric.id,
          sentenceCount: (metric.explainer.match(/[.!?](?:\s|$)/g) || []).length,
          sourceCount: metric.sources?.length || 0,
          sourceUrls: (metric.sources || []).map((source) => source.url),
        })),
      };
    }
    if (name === "raw-accelerometer-stacked") {
      app.outputs.add("raw_acc");
      renderOutputs();
      app.selectedVisual = "raw_acc";
      elements["visual-source"].value = app.selectedVisual;
      updateVisualLabels();
      resizeCanvas();
      for (const id of ["acc_x", "acc_y", "acc_z"]) ensureBuffer(id).clear();
      ingestAccelerometer(Array.from({ length: 360 }, (_, index) => ({
        xMg: Math.round(Math.sin(index / 15) * 850),
        yMg: Math.round(Math.cos(index / 23) * 420),
        zMg: Math.round(Math.sin(index / 31 + 0.8) * 1200),
      })));
      drawSignal();
      await new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));
      resizeCanvas();
      drawSignal();
      return {
        scenario: name,
        selectedVisual: app.selectedVisual,
        visualOptions: [...elements["visual-source"].options].map((option) => option.value),
        legendLabels: [...elements["visual-legend"].querySelectorAll("strong")].map((label) => label.textContent),
        currentLabel: elements["visual-current"].textContent,
        chartClass: elements["chart-shell"].className,
        canvasLabel: elements["signal-canvas"].getAttribute("aria-label"),
      };
    }
    if (name === "breathing-waveform-trail") {
      app.outputs.add("breathing_volume");
      app.metricOptions.breathing_volume = structuredClone(metricOptionFor("breathing_volume"));
      renderOutputs();
      app.selectedVisual = "breathing_volume";
      elements["visual-source"].value = app.selectedVisual;
      updateVisualLabels();
      resizeCanvas();
      const breathingBuffer = ensureBuffer("breathing_volume");
      breathingBuffer.clear();
      breathingBuffer.pushMany(Array.from({ length: 240 }, (_, index) => (
        0.5 + 0.36 * Math.sin(index * Math.PI * 2 / 80)
      )));
      drawSignal();
      await new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));
      resizeCanvas();
      drawSignal();
      return {
        scenario: name,
        selectedVisual: app.selectedVisual,
        chartClass: elements["chart-shell"].className,
        canvasLabel: elements["signal-canvas"].getAttribute("aria-label"),
        visualMode: elements["signal-canvas"].dataset.visualMode,
        direction: elements["signal-canvas"].dataset.breathDirection,
        latestY01: Number(elements["signal-canvas"].dataset.latestY01),
        trailPoints: Number(elements["signal-canvas"].dataset.trailPoints),
        currentLabel: elements["visual-current"].textContent,
      };
    }
    const targets = {
      "breathing-phase-inhale": { phase: 1, label: "INHALE" },
      "breathing-phase-exhale": { phase: -1, label: "EXHALE" },
      "breathing-phase-pause": { phase: 0, label: "PAUSE" },
    };
    const target = targets[name] || targets["breathing-phase-inhale"];
    app.outputs.add("breathing_phase");
    app.metricOptions.breathing_phase = structuredClone(metricOptionFor("breathing_phase"));
    renderOutputs();
    app.selectedVisual = "breathing_phase";
    elements["visual-source"].value = app.selectedVisual;
    updateVisualLabels();
    resizeCanvas();
    ensureBuffer("breathing_phase").clear();
    ensureBuffer("breathing_phase").push(target.phase);
    resetPhaseMotion(target.phase > 0 ? 0.18 : target.phase < 0 ? 0.82 : 0.58, target.phase === 0 ? 0.8 : 0);
    for (let frame = 0; frame < 180; frame += 1) {
      drawBreathingPhase(signalContext, elements["signal-canvas"], ensureBuffer("breathing_phase"), frame * (1000 / 60));
    }

    if (name === "breathing-phase-settings") {
      openModuleSettings("breathing_phase");
    } else if (elements["module-dialog"].open) {
      elements["module-dialog"].close();
    }
    await new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));
    resizeCanvas();
    drawBreathingPhase(signalContext, elements["signal-canvas"], ensureBuffer("breathing_phase"), 181 * (1000 / 60));
    return {
      scenario: name,
      expectedLabel: target.label,
      currentLabel: elements["visual-current"].textContent,
      selectedVisual: app.selectedVisual,
      streamName: streamOutputName(app.catalog.find((metric) => metric.id === "breathing_phase")),
      dialogOpen: elements["module-dialog"].open,
      phaseMotion: structuredClone(app.phaseMotion),
    };
  }

  const initialization = initialize();
  if (isInterfaceRenderer) {
    window.PolarInterfaceRenderer = Object.freeze({
      scenarios: Object.freeze([
        "breathing-phase-inhale",
        "breathing-phase-exhale",
        "breathing-phase-pause",
        "breathing-phase-settings",
        "breathing-waveform-trail",
        "raw-accelerometer-stacked",
        "multiple-colored-sources",
        "metric-library-previews",
      ]),
      ready: () => initialization,
      render: async (scenario) => {
        await initialization;
        return renderInterfaceScenario(scenario);
      },
      metricOptions: (id) => structuredClone(metricOptionFor(id)),
    });
  }
})();
