import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, stat } from "node:fs/promises";
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

async function connectMock(page) {
  const source = page.locator('.device-row[data-input-kind="mock"], .device-row.mock').first();
  await source.waitFor({ state: "visible" });
  await source.click();
  await page.locator("#input-state").filter({ hasText: "Demo live" }).waitFor();
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
    device.gatt = {
      async connect() { server.connected = true; return server; },
    };
    Object.defineProperty(navigator, "bluetooth", {
      configurable: true,
      value: {
        async requestDevice(options) {
          window.__polarFake.lastRequest = options;
          return device;
        },
      },
    });
    window.__polarFake = {
      writes,
      lastRequest: null,
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
  assert.match(await desktop.locator(".device-row.mock").textContent(), /SYNTHETIC/);
  assert.match(await desktop.locator(".device-row.mock").textContent(), /no Polar H10 required/);
  assert.equal(await desktop.locator("#browser-local-destination").isVisible(), true, "browser-local destination is missing");
  assert.equal(await desktop.locator("#lsl-destination-row").isHidden(), true, "browser mode must not offer native LSL");
  assert.equal(await desktop.locator("#osc-destination-row").isHidden(), true, "browser mode must not offer native OSC");
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
  assert.equal(await desktop.locator("#browser-record-button").isEnabled(), true, "browser recorder is unavailable after connecting");
  await desktop.locator("#browser-record-button").click();
  await desktop.locator("#browser-recorder-status").filter({ hasText: "REC" }).waitFor();
  await desktop.waitForFunction(() => window.PolarBrowserSession.status().rowCount >= 20);
  await desktop.locator("#browser-record-button").click();
  await desktop.locator("#browser-recorder-status").filter({ hasText: "FILE" }).waitFor();
  const [recordingDownload] = await Promise.all([
    desktop.waitForEvent("download"),
    desktop.locator("#browser-export-button").click(),
  ]);
  assert.match(recordingDownload.suggestedFilename(), /^Polar-H10_.*Z\.csv$/);
  const recordingPath = await recordingDownload.path();
  const recordingCsv = await readFile(recordingPath, "utf8");
  assert.match(recordingCsv, /^# Polar Stream browser recording/m);
  assert.match(recordingCsv, /host_timestamp_ms,relative_time_s,sensor_timestamp_ns,stream/);
  assert.match(recordingCsv, /,raw_ecg,/);
  assert.match(recordingCsv, /,raw_acc,/);
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
  const fixture = await desktop.evaluate(() => ({
    source: window.PolarDemoData?.source,
    ecgSamples: window.PolarDemoData?.ecg?.microvolts?.length,
    accSamples: window.PolarDemoData?.accelerometer?.milligravity?.[0]?.length,
  }));
  assert.equal(fixture.source.library, "NeuroKit2");
  assert.deepEqual(fixture.source.models, ["ECGSYN", "RSP"]);
  assert.equal(fixture.ecgSamples, 3900);
  assert.equal(fixture.accSamples, 6000);
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
  await webBluetoothRow.click();
  await bluetooth.locator("#input-state").filter({ hasText: "Browser BLE live" }).waitFor();
  assert.equal(await bluetooth.locator("#battery-value").textContent(), "87%");
  assert.equal(await bluetooth.locator("#runtime-path-label").textContent(), "Browser Bluetooth · experimental");
  const bluetoothContract = await bluetooth.evaluate(() => ({
    writes: window.__polarFake.writes,
    request: window.__polarFake.lastRequest,
  }));
  assert.deepEqual(bluetoothContract.writes.slice(0, 2), [
    [0x02, 0x00, 0x00, 0x01, 130, 0, 0x01, 0x01, 14, 0],
    [0x02, 0x02, 0x02, 0x01, 8, 0, 0x00, 0x01, 200, 0, 0x01, 0x01, 16, 0],
  ]);
  assert.deepEqual(bluetoothContract.request.filters, [{ namePrefix: "Polar H10" }]);
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
  });
  await phone.goto(baseUrl, { waitUntil: "networkidle" });
  await assertNoHorizontalOverflow(phone, "phone browser demo before connection");
  const panelTops = await phone.locator(".workspace-panel").evaluateAll((panels) => panels.map((panel) => panel.getBoundingClientRect().top));
  assert.ok(panelTops[0] < panelTops[1] && panelTops[1] < panelTops[2], `phone panels are not stacked: ${panelTops}`);
  const scanBox = await phone.locator("#scan-button").boundingBox();
  assert.ok(scanBox.height >= 44, `phone primary action is too short: ${scanBox.height}`);
  await connectMock(phone);

  await phone.locator("#open-output-dialog").scrollIntoViewIfNeeded();
  await phone.locator("#open-output-dialog").click();
  await phone.getByRole("button", { name: /ACC metrics/ }).click();
  await phone.locator('.metric-option[data-metric-id="breathing_phase"]').click();
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

  process.stdout.write(`Validated canonical Pages parity, browser-only runtime isolation, NeuroKit replay, Web Bluetooth PMD, and responsive layouts in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
