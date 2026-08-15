import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { chromium } from "playwright";

const repository = normalize(new URL("../", import.meta.url).pathname);
const root = normalize(new URL("../artifacts/browser-demo/", import.meta.url).pathname);
const output = normalize(new URL("../artifacts/browser-demo-validation/", import.meta.url).pathname);
const mime = new Map([
  [".html", "text/html; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".png", "image/png"],
]);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function startServer() {
  const server = createServer(async (request, response) => {
    try {
      const requestPath = new URL(request.url, "http://browser-demo.local").pathname;
      const relative = requestPath === "/" ? "index.html" : requestPath.slice(1);
      const path = normalize(join(root, relative));
      if (!path.startsWith(root)) throw new Error("Path outside browser demo root");
      const body = await readFile(path);
      response.writeHead(200, {
        "content-type": mime.get(extname(path)) || "application/octet-stream",
        "cache-control": "no-store",
        "content-security-policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'",
      });
      response.end(body);
    } catch (_error) {
      response.writeHead(404);
      response.end("Not found");
    }
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

async function assertNoHorizontalOverflow(page, label) {
  const dimensions = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  assert.ok(
    dimensions.scrollWidth <= dimensions.clientWidth + 1,
    `${label} has horizontal page overflow: ${JSON.stringify(dimensions)}`,
  );
}

function stereoPcmWav(left, right, sampleRate) {
  const frames = Math.min(left.length, right.length);
  const dataLength = frames * 4;
  const wav = Buffer.alloc(44 + dataLength);
  wav.write("RIFF", 0);
  wav.writeUInt32LE(36 + dataLength, 4);
  wav.write("WAVEfmt ", 8);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(2, 22);
  wav.writeUInt32LE(sampleRate, 24);
  wav.writeUInt32LE(sampleRate * 4, 28);
  wav.writeUInt16LE(4, 32);
  wav.writeUInt16LE(16, 34);
  wav.write("data", 36);
  wav.writeUInt32LE(dataLength, 40);
  for (let index = 0; index < frames; index += 1) {
    wav.writeInt16LE(Math.round(Math.max(-1, Math.min(1, left[index])) * 32767), 44 + index * 4);
    wav.writeInt16LE(Math.round(Math.max(-1, Math.min(1, right[index])) * 32767), 46 + index * 4);
  }
  return wav;
}

async function connectMock(page) {
  const source = page.locator('.device-row[data-input-kind="mock"], .device-row.mock').first();
  await source.waitFor({ state: "visible" });
  await source.click();
  await page.locator("#input-state").filter({ hasText: "Recorded preview looping" }).waitFor();
  await page.waitForFunction(() => document.querySelector("#sample-counter")?.textContent !== "0 samples");
  assert.equal(await page.locator("#chart-empty").isHidden(), true, "mock input did not activate the live chart");
  assert.notEqual(await page.locator("#raw-ecg-value").textContent(), "—", "mock ECG did not update");
  assert.notEqual(await page.locator("#raw-acc-z").textContent(), "—", "mock ACC did not update");
}

async function installFakeWebBluetooth(page) {
  await page.addInitScript(() => {
    const uuids = {
      pmdService: "fb005c80-02e7-f387-1cad-8acd2d8df0c8",
      pmdControl: "fb005c81-02e7-f387-1cad-8acd2d8df0c8",
      pmdData: "fb005c82-02e7-f387-1cad-8acd2d8df0c8",
      heartRateService: "0000180d-0000-1000-8000-00805f9b34fb",
      heartRateMeasurement: "00002a37-0000-1000-8000-00805f9b34fb",
      batteryService: "0000180f-0000-1000-8000-00805f9b34fb",
      batteryLevel: "00002a19-0000-1000-8000-00805f9b34fb",
    };
    const writes = [];
    class FakeCharacteristic extends EventTarget {
      constructor(uuid, readBytes = []) {
        super();
        this.uuid = uuid;
        this.readBytes = readBytes;
        this.value = new DataView(new ArrayBuffer(0));
      }
      async startNotifications() { return this; }
      async stopNotifications() { return this; }
      async writeValueWithResponse(value) { writes.push(Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength))); }
      async writeValue(value) { writes.push(Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength))); }
      async readValue() {
        const bytes = Uint8Array.from(this.readBytes);
        return new DataView(bytes.buffer);
      }
      emit(bytes) {
        const value = Uint8Array.from(bytes);
        this.value = new DataView(value.buffer);
        this.dispatchEvent(new Event("characteristicvaluechanged"));
      }
    }
    const characteristics = {
      control: new FakeCharacteristic(uuids.pmdControl),
      pmd: new FakeCharacteristic(uuids.pmdData),
      heartRate: new FakeCharacteristic(uuids.heartRateMeasurement),
      battery: new FakeCharacteristic(uuids.batteryLevel, [87]),
    };
    const services = new Map([
      [uuids.pmdService, new Map([
        [uuids.pmdControl, characteristics.control],
        [uuids.pmdData, characteristics.pmd],
      ])],
      [uuids.heartRateService, new Map([[uuids.heartRateMeasurement, characteristics.heartRate]])],
      [uuids.batteryService, new Map([[uuids.batteryLevel, characteristics.battery]])],
    ]);
    const server = {
      connected: false,
      async getPrimaryService(uuid) {
        const service = services.get(String(uuid).toLowerCase());
        if (!service) throw new DOMException("Service unavailable", "NotFoundError");
        return {
          async getCharacteristic(characteristicUuid) {
            const characteristic = service.get(String(characteristicUuid).toLowerCase());
            if (!characteristic) throw new DOMException("Characteristic unavailable", "NotFoundError");
            return characteristic;
          },
        };
      },
      disconnect() {
        this.connected = false;
        device.dispatchEvent(new Event("gattserverdisconnected"));
      },
    };
    const device = new EventTarget();
    device.name = "Polar H10 TEST1234";
    device.id = "fake-polar-h10";
    let failGattConnectAttempts = 0;
    let gattConnectAttempts = 0;
    device.gatt = {
      async connect() {
        gattConnectAttempts += 1;
        if (failGattConnectAttempts > 0) {
          failGattConnectAttempts -= 1;
          throw new DOMException("Transient GATT connection failure", "NetworkError");
        }
        server.connected = true;
        return server;
      },
    };
    let cancelNextChooser = false;
    let disableNextChooser = false;
    let blockNextChooserByPolicy = false;
    let activationAtRequest = null;
    Object.defineProperty(navigator, "bluetooth", {
      configurable: true,
      value: {
        async requestDevice(options) {
          window.__polarFake.lastRequest = options;
          activationAtRequest = navigator.userActivation?.isActive ?? null;
          if (cancelNextChooser) {
            cancelNextChooser = false;
            throw new DOMException("User cancelled the chooser", "NotFoundError");
          }
          if (disableNextChooser) {
            disableNextChooser = false;
            throw new DOMException("Web Bluetooth API globally disabled.", "NotFoundError");
          }
          if (blockNextChooserByPolicy) {
            blockNextChooserByPolicy = false;
            throw new DOMException("Web Bluetooth is not allowed by permissions policy.", "NotAllowedError");
          }
          return device;
        },
      },
    });
    const wakeLockSentinel = new EventTarget();
    wakeLockSentinel.released = false;
    wakeLockSentinel.release = async () => {
      wakeLockSentinel.released = true;
      wakeLockSentinel.dispatchEvent(new Event("release"));
    };
    Object.defineProperty(navigator, "wakeLock", {
      configurable: true,
      value: {
        async request(type) {
          window.__polarFake.wakeLockRequests.push(type);
          wakeLockSentinel.released = false;
          return wakeLockSentinel;
        },
      },
    });
    window.__polarFake = {
      writes,
      wakeLockRequests: [],
      lastRequest: null,
      get activationAtRequest() { return activationAtRequest; },
      get gattConnectAttempts() { return gattConnectAttempts; },
      cancelNextChooser() { cancelNextChooser = true; },
      disableNextChooser() { disableNextChooser = true; },
      blockNextChooserWithPolicy() { blockNextChooserByPolicy = true; },
      failNextGattConnect() { failGattConnectAttempts = 1; },
      useLegacyControlWrites() { characteristics.control.writeValueWithResponse = undefined; },
      emitPmd(bytes) { characteristics.pmd.emit(bytes); },
      emitHeartRate(bytes) { characteristics.heartRate.emit(bytes); },
    };
  });
}

await mkdir(output, { recursive: true });
const manifest = JSON.parse(await readFile(join(root, "browser-demo-manifest.json"), "utf8"));
assert.equal(manifest.canonicalSource, "apps/polar-stream/ui");
for (const [name, expected] of Object.entries(manifest.sha256)) {
  const canonical = await readFile(join(repository, manifest.canonicalSource, name));
  const staged = await readFile(join(root, name));
  assert.equal(sha256(canonical), expected, `${name} canonical hash differs from the manifest`);
  assert.equal(sha256(staged), expected, `${name} Pages artifact differs from the canonical UI`);
}

const server = await startServer();
const address = server.address();
const baseUrl = `http://127.0.0.1:${address.port}/`;
const browser = await chromium.launch({ headless: true });

try {
  const desktop = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
  await desktop.goto(baseUrl, { waitUntil: "networkidle" });
  assert.equal(await desktop.locator("body").getAttribute("data-runtime"), "browser-demo");
  assert.equal(await desktop.locator("#platform-label").textContent(), "BROWSER DEMO");
  assert.equal(await desktop.locator("#runtime-path-label").textContent(), "Browser-local inputs");
  assert.match(await desktop.locator(".device-row.mock").textContent(), /RECORDED/);
  assert.match(await desktop.locator(".device-row.mock").textContent(), /seamless loop of an anonymized 60-second ECG \+ ACC recording/i);
  assert.equal(await desktop.locator("#browser-local-destination").isVisible(), true, "browser-local destination is missing");
  assert.equal(await desktop.locator("#csv-destination-row").isVisible(), true, "local CSV toggle is missing");
  assert.equal(await desktop.locator("#audio-destination-row").isVisible(), true, "audio-data toggle is missing");
  assert.equal(await desktop.locator("#lsl-destination-row").isVisible(), true, "shared LSL control is missing in browser mode");
  assert.equal(await desktop.locator("#osc-destination-row").isVisible(), true, "shared OSC control is missing in browser mode");
  assert.equal(await desktop.locator("#lsl-toggle").isEnabled(), true, "browser LSL control must remain interactive so it can explain the limitation");
  assert.equal(await desktop.locator("#osc-toggle").isEnabled(), true, "browser OSC control must remain interactive so it can explain the limitation");
  await desktop.locator("#lsl-destination-row").click();
  assert.equal(await desktop.locator("#lsl-toggle").isChecked(), false, "browser LSL must fail closed");
  assert.equal(await desktop.locator("#native-output-browser-error").isVisible(), true, "browser LSL refusal did not surface an inline error");
  assert.match(await desktop.locator("#native-output-browser-error").textContent(), /LSL output.*installed Polar Stream app/i);
  assert.equal(
    await desktop.locator("#native-output-browser-error a").getAttribute("href"),
    "https://github.com/GeorgeFejer91/Polar-Stream/releases/latest",
    "installed-app error does not link to the latest release",
  );
  await desktop.locator("#osc-destination-row").click();
  assert.equal(await desktop.locator("#osc-toggle").isChecked(), false, "browser OSC must fail closed");
  assert.match(await desktop.locator("#native-output-browser-error").textContent(), /OSC output.*installed Polar Stream app/i);
  const nativeOutputRejection = await desktop.evaluate(async () => {
    try {
      await window.PolarRuntimeApi.updateOutputConfig({
        streamName: "Browser rejection test",
        lslEnabled: true,
        oscEnabled: false,
        csvEnabled: false,
        audioEnabled: false,
        outputs: ["raw_ecg", "raw_acc"],
        metricOptions: {},
      });
      return null;
    } catch (error) {
      return { code: error.code, message: error.message };
    }
  });
  assert.equal(nativeOutputRejection.code, "NATIVE_OUTPUT_REQUIRES_APP");
  assert.match(nativeOutputRejection.message, /installed Polar Stream app/i);
  assert.equal(await desktop.evaluate(() => "PolarBrowserLslBridge" in window), false, "browser build still exposes the native bridge adapter");
  await desktop.evaluate(() => {
    window.__polarBrowserEvents = 0;
    window.__polarChannelEvents = 0;
    window.addEventListener("polar-stream-data", () => { window.__polarBrowserEvents += 1; });
    window.__polarChannelReceiver = new BroadcastChannel(window.PolarBrowserSession.channelName);
    window.__polarChannelReceiver.addEventListener("message", () => { window.__polarChannelEvents += 1; });
  });
  await connectMock(desktop);
  await desktop.waitForFunction(() => window.__polarBrowserEvents > 3 && window.__polarChannelEvents > 3);
  await desktop.locator("#csv-destination-row").click();
  await desktop.locator("#browser-recorder-status").filter({ hasText: "REC" }).waitFor();
  await desktop.waitForFunction(() => window.PolarBrowserSession.status().rowCount >= 20);
  const [recordingDownload] = await Promise.all([
    desktop.waitForEvent("download"),
    desktop.locator("#csv-destination-row").click(),
  ]);
  assert.match(recordingDownload.suggestedFilename(), /^Polar-H10_.*Z\.csv$/);
  const recordingPath = await recordingDownload.path();
  const recordingCsv = await readFile(recordingPath, "utf8");
  assert.match(recordingCsv, /^# Polar Stream browser recording/m);
  assert.match(recordingCsv, /host_timestamp_ms,relative_time_s,sensor_timestamp_ns,stream/);
  assert.match(recordingCsv, /,raw_ecg,/);
  assert.match(recordingCsv, /,raw_acc,/);
  assert.equal(await desktop.locator("#browser-recorder-status").textContent(), "READY");
  const boundedRecorder = await desktop.evaluate(async () => {
    let now = 1_000;
    const recorder = window.PolarBrowserSession.createRecorder({ maxRows: 2, now: () => now });
    recorder.configure({ streamName: "Bounded test", outputs: ["raw_ecg"] });
    recorder.start({ deviceName: "Fixture", inputKind: "mock" });
    now = 1_100;
    recorder.capture({ kind: "ecg", sensorTimestampNs: "1000000000", microvolts: [1, 2, 3] }, now);
    const status = recorder.snapshot();
    const csv = await recorder.createBlob().text();
    return { status, csv };
  });
  assert.equal(boundedRecorder.status.state, "stopped");
  assert.equal(boundedRecorder.status.stopReason, "capacity");
  assert.equal(boundedRecorder.status.rowCount, 2);
  assert.match(boundedRecorder.csv, /,raw_ecg,0,,,,1,uV/);
  const audioFixture = await desktop.evaluate(() => {
    const packet = window.PolarAudioDataLink.encodeBatch({
      ecg: [1, -2, 3],
      ecgTimestamp: "1000000000",
      accelerometer: [{ xMg: 4, yMg: -5, zMg: 6 }],
      accTimestamp: "2000000000",
      metrics: [{ id: "heart_rate", value: 61 }],
    }, 7);
    const decoded = window.PolarAudioDataLink.decodePacket(packet);
    const waveform = window.PolarAudioDataLink.packetWaveform(packet, 44_100);
    return {
      packet: Array.from(packet),
      sequence: decoded.sequence,
      schemaVersion: decoded.schemaVersion,
      left: Array.from(waveform.left),
      right: Array.from(waveform.right),
    };
  });
  assert.equal(audioFixture.sequence, 7);
  assert.equal(audioFixture.schemaVersion, 1);
  const audioWav = join(output, "audio-data-link-fixture.wav");
  const decodedAudioCsv = join(output, "audio-data-link-fixture.decoded.csv");
  await writeFile(audioWav, stereoPcmWav(audioFixture.left, audioFixture.right, 44_100));
  const decoder = spawnSync("python3", [
    join(repository, "scripts/decode_audio_data.py"),
    audioWav,
    "--output",
    decodedAudioCsv,
  ], { encoding: "utf8" });
  assert.equal(decoder.status, 0, `audio reference decoder failed: ${decoder.stderr}`);
  const decodedAudio = await readFile(decodedAudioCsv, "utf8");
  assert.match(decodedAudio, /7,.*raw_ecg,0,,,,1,uV/);
  assert.match(decodedAudio, /7,.*raw_acc,0,4,-5,6,,mg/);
  assert.match(decodedAudio, /7,,heart_rate,0,,,,61(?:\.0)?,bpm/);
  const fixture = await desktop.evaluate(async () => {
    const recording = await fetch("data/preview-recording.json").then((response) => response.json());
    return {
      source: recording.source,
      durationMs: recording.durationMs,
      ecgSamples: recording.ecg.microvolts.length,
      accSamples: recording.accelerometer.samples.length,
    };
  });
  assert.equal(fixture.source, "real-polar-h10-recording");
  assert.equal(fixture.durationMs, 60_000);
  assert.equal(fixture.ecgSamples, 7_800);
  assert.equal(fixture.accSamples, 12_000);
  await desktop.locator("#open-output-dialog").click();
  await desktop.locator('.metric-option[data-metric-id="rmssd"]').click();
  assert.match(await desktop.locator(".metric-preview-provenance").textContent(), /Recorded Polar H10/);
  assert.match(await desktop.locator(".metric-formula-context").textContent(), /RMSSD =/);
  assert.match(await desktop.locator("#metric-detail").textContent(), /Current scientific view/);
  assert.match(await desktop.locator(".metric-source a").getAttribute("href"), /^https:\/\//);
  const originalPath = await desktop.locator(".metric-preview-large .metric-preview-line").first().getAttribute("d");
  await desktop.getByLabel("Published scale").selectOption("slidingWindow");
  await desktop.getByLabel("Normalization window").fill("10");
  await desktop.waitForFunction((before) => document.querySelector(".metric-preview-large .metric-preview-line")?.getAttribute("d") !== before, originalPath);
  const transformedPath = await desktop.locator(".metric-preview-large .metric-preview-line").first().getAttribute("d");
  assert.notEqual(transformedPath, originalPath, "metric settings did not update the recorded outcome preview");
  await desktop.getByRole("button", { name: "Open editable formula + preview" }).click();
  assert.equal(await desktop.locator("#formula-dialog").isVisible(), true);
  assert.match(await desktop.locator(".formula-variable-map").textContent(), /Time is retained automatically|time is retained automatically/i);
  assert.equal(await desktop.locator('#formula-keyboard button[title]').count() > 10, true, "formula calculator keys lack hover help");
  await desktop.locator("#formula-validation-status").filter({ hasText: "Valid" }).waitFor();
  assert.notEqual(await desktop.locator("#formula-preview-current").textContent(), "—");
  assert.equal(await desktop.locator("#formula-preview-canvas").getAttribute("data-looping"), "true", "Formula Lab preview is not continuously looped");
  await desktop.locator("#save-custom-formula").click();
  await desktop.locator(".formula-output-card").filter({ hasText: "rmssd_custom" }).waitFor();
  await desktop.locator("#open-output-dialog").click();
  await desktop.getByRole("button", { name: /ACC metrics/ }).click();
  await desktop.locator('.metric-option[data-metric-id="breathing_phase"]').click();
  await desktop.locator("#save-metric-output").click();
  await desktop.locator("#visual-source").selectOption("breathing_phase");
  await desktop.waitForFunction(() => /INHALE|EXHALE|PAUSE/.test(document.querySelector("#visual-current")?.textContent || ""));
  assert.match(await desktop.locator("#chart-shell").getAttribute("class"), /phase-visual/);
  await desktop.waitForFunction(() => !document.querySelector(".toast"));
  await assertNoHorizontalOverflow(desktop, "desktop browser demo");
  const desktopScreenshot = join(output, "browser-demo-desktop.png");
  await desktop.screenshot({ path: desktopScreenshot, fullPage: true });
  assert.ok((await stat(desktopScreenshot)).size > 30_000, "desktop browser-demo screenshot is unexpectedly empty");
  await desktop.close();

  const bluetooth = await browser.newPage({ viewport: { width: 1280, height: 820 }, deviceScaleFactor: 1 });
  await installFakeWebBluetooth(bluetooth);
  await bluetooth.goto(baseUrl, { waitUntil: "networkidle" });
  const webBluetoothRow = bluetooth.locator('.device-row[data-input-kind="web-bluetooth"]');
  assert.match(await webBluetoothRow.textContent(), /EXPERIMENTAL/);
  assert.match(await webBluetoothRow.textContent(), /Choose H10/);
  assert.equal(await bluetooth.locator("#scan-button span").textContent(), "Choose Polar H10");
  await bluetooth.evaluate(() => window.__polarFake.cancelNextChooser());
  await bluetooth.locator("#scan-button").click();
  await bluetooth.locator("#input-state").filter({ hasText: "Browser ready" }).waitFor();
  assert.equal(await bluetooth.locator("#app-state-text").textContent(), "Browser inputs ready");
  assert.match(await bluetooth.locator("#connection-detail").textContent(), /No sensor was selected/);
  assert.equal(await bluetooth.locator(".toast.error").count(), 0, "chooser cancellation must not create an error toast");
  assert.equal(await bluetooth.locator("#scan-button span").textContent(), "Choose Polar H10");
  await bluetooth.evaluate(() => window.__polarFake.disableNextChooser());
  await bluetooth.locator("#scan-button").click();
  await bluetooth.locator("#input-state").filter({ hasText: "Error" }).waitFor();
  assert.equal(await bluetooth.locator("#app-state-text").textContent(), "Connection failed");
  assert.match(await bluetooth.locator("#connection-detail").textContent(), /browser blocks Web Bluetooth/i);
  assert.equal(await bluetooth.locator(".toast.error").count(), 1, "a browser-level Bluetooth block must be visible as an error");
  await bluetooth.waitForFunction(() => !document.querySelector(".toast"));
  await bluetooth.evaluate(() => window.__polarFake.blockNextChooserWithPolicy());
  await bluetooth.locator("#scan-button").click();
  await bluetooth.locator("#input-state").filter({ hasText: "Error" }).waitFor();
  assert.match(await bluetooth.locator("#connection-detail").textContent(), /embedding policy blocks Web Bluetooth/i);
  await bluetooth.waitForFunction(() => !document.querySelector(".toast"));
  await bluetooth.evaluate(() => {
    window.__polarFake.failNextGattConnect();
    window.__polarFake.useLegacyControlWrites();
  });
  await bluetooth.locator("#scan-button").click();
  await bluetooth.locator("#input-state").filter({ hasText: "Browser BLE live" }).waitFor();
  assert.equal(await bluetooth.locator("#battery-value").textContent(), "87%");
  assert.equal(await bluetooth.locator("#runtime-path-label").textContent(), "Browser Bluetooth · experimental");
  const bluetoothContract = await bluetooth.evaluate(() => ({
    writes: window.__polarFake.writes,
    request: window.__polarFake.lastRequest,
    wakeLockRequests: window.__polarFake.wakeLockRequests,
    activationAtRequest: window.__polarFake.activationAtRequest,
    gattConnectAttempts: window.__polarFake.gattConnectAttempts,
  }));
  assert.deepEqual(bluetoothContract.writes.slice(0, 2), [
    [0x02, 0x00, 0x00, 0x01, 130, 0, 0x01, 0x01, 14, 0],
    [0x02, 0x02, 0x02, 0x01, 8, 0, 0x00, 0x01, 200, 0, 0x01, 0x01, 16, 0],
  ]);
  assert.deepEqual(bluetoothContract.request.filters, [{ namePrefix: "Polar H10" }]);
  assert.deepEqual(bluetoothContract.request.optionalServices, [
    "fb005c80-02e7-f387-1cad-8acd2d8df0c8",
    "0000180d-0000-1000-8000-00805f9b34fb",
    "0000180f-0000-1000-8000-00805f9b34fb",
  ]);
  assert.equal(bluetoothContract.activationAtRequest, true, "chooser lost its initiating user activation");
  assert.equal(bluetoothContract.gattConnectAttempts, 2, "transient GATT failure was not retried exactly once");
  assert.deepEqual(bluetoothContract.wakeLockRequests, ["screen"]);
  await bluetooth.evaluate(() => {
    window.__polarFake.emitPmd([0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 100, 0, 0, 156, 255, 255]);
    window.__polarFake.emitPmd([0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 1, 0, 254, 255, 3, 0]);
    window.__polarFake.emitHeartRate([0x10, 60, 0x00, 0x04]);
  });
  await bluetooth.waitForFunction(() => (
    document.querySelector("#raw-ecg-value")?.textContent === "-100"
    && document.querySelector("#raw-acc-x")?.textContent === "1"
  ));
  assert.equal(await bluetooth.locator("#raw-ecg-value").textContent(), "-100");
  assert.equal(await bluetooth.locator("#raw-acc-x").textContent(), "1");
  const protocolChecks = await bluetooth.evaluate(() => {
    const decoded = window.PolarWebBluetooth.decodePmd(Uint8Array.from([
      0x00, 42, 0, 0, 0, 0, 0, 0, 0, 0x00,
      1, 0, 0, 255, 255, 255, 0, 0, 128,
    ]));
    const compressed = window.PolarWebBluetooth.decodePmd(Uint8Array.from([
      0x02, 7, 0, 0, 0, 0, 0, 0, 0, 0x81,
      0xe8, 0x03, 0xfe, 0xff, 0x03, 0x00,
      0x04, 0x02, 0xf1, 0xe2, 0xf0,
    ]));
    let malformedCode = null;
    try { window.PolarWebBluetooth.decodePmd(Uint8Array.from([0])); } catch (error) { malformedCode = error.code; }
    const processor = window.PolarWebBluetooth.createBreathingProcessor({
      axes: [true, false, true],
      calibrationWindowSeconds: 1,
      minimumAxisRangeG: 0.001,
      smoothingWindowSeconds: 0.05,
    });
    let snapshot = null;
    for (let block = 0; block < 12; block += 1) {
      const samples = Array.from({ length: 20 }, (_, offset) => {
        const index = block * 20 + offset;
        return { xMg: 0, yMg: 0, zMg: 1000 + Math.round(30 * Math.sin(index / 200 * Math.PI * 2 * 0.25)) };
      });
      snapshot = processor.push(samples, block * 100);
    }
    return { decoded, compressed, malformedCode, snapshot };
  });
  assert.deepEqual(protocolChecks.decoded.microvolts, [1, -1, -8388608]);
  assert.equal(protocolChecks.decoded.sensorTimestampNs, "42");
  assert.deepEqual(protocolChecks.compressed.samples, [
    { xMg: 1000, yMg: -2, zMg: 3 },
    { xMg: 1001, yMg: -3, zMg: 5 },
    { xMg: 999, yMg: -3, zMg: 4 },
  ]);
  assert.equal(protocolChecks.malformedCode, "PMD_FRAME_TOO_SHORT");
  assert.equal(protocolChecks.snapshot.calibrated, true, "browser breathing processor did not calibrate");
  assert.ok(protocolChecks.snapshot.axisRangeG >= 0.001, "browser breathing range is too small");
  await bluetooth.locator("#open-output-dialog").click();
  await bluetooth.locator('.metric-option[data-metric-id="ecg_mean"]').click();
  assert.equal(await bluetooth.locator("#save-metric-output").textContent(), "Desktop only");
  assert.match(await bluetooth.locator("#dialog-output-status").textContent(), /requires the desktop app/);
  await assertNoHorizontalOverflow(bluetooth, "desktop Web Bluetooth input");
  await bluetooth.close();

  const phone = await browser.newPage({
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 1,
    hasTouch: true,
    isMobile: true,
    userAgent: "Mozilla/5.0 (Linux; Android 14; Moto G) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36",
  });
  await installFakeWebBluetooth(phone);
  await phone.goto(baseUrl, { waitUntil: "networkidle" });
  await assertNoHorizontalOverflow(phone, "phone browser demo before connection");
  assert.equal(await phone.locator("#lsl-destination-row").isVisible(), true, "phone browser UI hid the shared LSL control");
  assert.equal(await phone.locator("#osc-destination-row").isVisible(), true, "phone browser UI hid the shared OSC control");
  await phone.locator("#lsl-destination-row").click();
  assert.equal(await phone.locator("#lsl-toggle").isChecked(), false, "phone browser LSL must fail closed");
  assert.equal(await phone.locator("#native-output-browser-error").isVisible(), true, "phone browser LSL refusal is not visible");
  const downloadLinkBox = await phone.locator("#native-output-browser-error a").boundingBox();
  assert.ok(downloadLinkBox.height >= 44, `phone installed-app download link is too short: ${downloadLinkBox.height}`);
  await phone.locator(".toast").evaluateAll((toasts) => toasts.forEach((toast) => toast.remove()));
  const panelTops = await phone.locator(".workspace-panel").evaluateAll((panels) => panels.map((panel) => panel.getBoundingClientRect().top));
  assert.ok(panelTops[0] < panelTops[1] && panelTops[1] < panelTops[2], `phone panels are not stacked: ${panelTops}`);
  const scanBox = await phone.locator("#scan-button").boundingBox();
  assert.ok(scanBox.height >= 44, `phone primary action is too short: ${scanBox.height}`);
  await phone.evaluate(() => window.__polarFake.disableNextChooser());
  await phone.locator("#scan-button").click();
  await phone.locator("#input-state").filter({ hasText: "Error" }).waitFor();
  assert.match(await phone.locator("#connection-detail").textContent(), /Google Chrome on Android/);
  await phone.waitForFunction(() => !document.querySelector(".toast"));
  await phone.locator('.device-row[data-input-kind="web-bluetooth"]').click();
  await phone.locator("#input-state").filter({ hasText: "Browser BLE live" }).waitFor();
  assert.deepEqual(await phone.evaluate(() => window.__polarFake.wakeLockRequests), ["screen"]);
  await phone.locator("#disconnect-button").click();
  await phone.locator("#input-state").filter({ hasText: "Browser ready" }).waitFor();
  await connectMock(phone);

  await phone.locator("#open-output-dialog").scrollIntoViewIfNeeded();
  await phone.locator("#open-output-dialog").click();
  assert.equal(await phone.locator("#output-dialog").getAttribute("data-mobile-view"), "browse");
  assert.equal(await phone.locator("#metric-detail").isHidden(), true, "phone browse view must prioritize the signal list");
  const familyButtons = await phone.locator(".family-choice").evaluateAll((buttons) => buttons.map((button) => ({
    width: button.getBoundingClientRect().width,
    height: button.getBoundingClientRect().height,
    top: button.getBoundingClientRect().top,
  })));
  assert.equal(familyButtons.length, 2);
  assert.ok(familyButtons.every(({ width, height }) => width >= 150 && height >= 44), `phone family controls are not touch friendly: ${JSON.stringify(familyButtons)}`);
  assert.ok(Math.abs(familyButtons[0].top - familyButtons[1].top) < 1, `phone family controls are not side by side: ${JSON.stringify(familyButtons)}`);
  await phone.getByRole("button", { name: /ACC metrics/ }).click();
  const searchBox = await phone.locator("#metric-search").boundingBox();
  assert.ok(searchBox.height >= 44, `phone metric search is too short: ${searchBox.height}`);
  const optionBoxes = await phone.locator(".metric-option").evaluateAll((options) => options.map((option) => ({
    width: option.getBoundingClientRect().width,
    height: option.getBoundingClientRect().height,
  })));
  assert.ok(optionBoxes.every(({ width, height }) => width >= 290 && height >= 72), `phone metric choices are not touch friendly: ${JSON.stringify(optionBoxes)}`);
  const phoneBrowseScreenshot = join(output, "browser-demo-phone-library-browse.png");
  await phone.screenshot({ path: phoneBrowseScreenshot, fullPage: false });
  assert.ok((await stat(phoneBrowseScreenshot)).size > 20_000, "phone browser-demo browse screenshot is unexpectedly empty");
  await phone.locator('.metric-option[data-metric-id="breathing_phase"]').click();
  assert.equal(await phone.locator("#output-dialog").getAttribute("data-mobile-view"), "detail");
  assert.equal(await phone.locator("#metric-options").isHidden(), true, "phone detail view must hide the browse list");
  assert.equal(await phone.locator("#metric-detail").isVisible(), true, "phone detail view did not open");
  assert.equal(await phone.getByRole("button", { name: "Back to all signals" }).isVisible(), true);
  assert.equal(await phone.getByLabel("X · recommended").isChecked(), true);
  assert.equal(await phone.getByLabel("Y · rotational").isChecked(), false);
  assert.equal(await phone.getByLabel("Z · recommended").isChecked(), true);
  assert.equal(await phone.getByLabel("Sensitivity").inputValue(), "0.6");
  await phone.getByText("Advanced experiment parameters").click();
  assert.equal(await phone.getByLabel("Calibration window").inputValue(), "12");
  assert.equal(await phone.getByLabel("Minimum axis range").inputValue(), "0.01");
  assert.equal(await phone.getByLabel("Adaptive bounds").isChecked(), true);
  assert.equal(await phone.getByLabel("Lower quantile").inputValue(), "0.05");
  assert.equal(await phone.getByLabel("Upper quantile").inputValue(), "0.95");
  await phone.getByRole("button", { name: "Back to all signals" }).click();
  assert.equal(await phone.locator("#output-dialog").getAttribute("data-mobile-view"), "browse");
  assert.equal(await phone.locator('.metric-option[data-metric-id="breathing_phase"]').getAttribute("aria-pressed"), "true");
  await phone.locator('.metric-option[data-metric-id="breathing_phase"]').click();
  const dialogBox = await phone.locator("#output-dialog").boundingBox();
  assert.ok(dialogBox.width >= 389 && dialogBox.height >= 843, `phone dialog is not visual-viewport sized: ${JSON.stringify(dialogBox)}`);
  await assertNoHorizontalOverflow(phone, "phone output library");
  const phoneScreenshot = join(output, "browser-demo-phone-library.png");
  await phone.screenshot({ path: phoneScreenshot, fullPage: false });
  assert.ok((await stat(phoneScreenshot)).size > 20_000, "phone browser-demo screenshot is unexpectedly empty");
  await phone.close();

  const narrow = await browser.newPage({ viewport: { width: 320, height: 720 }, hasTouch: true, isMobile: true });
  await narrow.goto(baseUrl, { waitUntil: "networkidle" });
  await assertNoHorizontalOverflow(narrow, "320px browser demo");
  assert.ok(await narrow.locator(".device-row.mock").isVisible(), "mock input is not visible at 320px");
  await narrow.close();

  process.stdout.write(`Validated canonical Pages parity, recorded H10 replay, Formula Lab, Web Bluetooth PMD, and responsive layouts in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
