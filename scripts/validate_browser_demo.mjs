import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const repository = normalize(fileURLToPath(new URL("../", import.meta.url)));
const root = normalize(fileURLToPath(new URL("../artifacts/browser-demo/", import.meta.url)));
const output = normalize(
  fileURLToPath(new URL("../artifacts/browser-demo-validation/", import.meta.url)),
);
const mime = new Map([
  [".html", "text/html; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".png", "image/png"],
]);
const textAssetSuffixes = new Set([".cjs", ".css", ".html", ".js", ".json", ".md", ".txt"]);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function canonicalAssetBytes(value, name) {
  if (!textAssetSuffixes.has(extname(name).toLowerCase())) return value;
  return Buffer.from(value.toString("utf8").replace(/\r\n/g, "\n"), "utf8");
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

async function installFakeVernierWebBluetooth(page) {
  await page.addInitScript(() => {
    const uuids = {
      service: "d91714ef-28b9-4f91-ba16-f0d9a604f112",
      command: "f4bf14a6-c7d5-4b6d-8aa8-df1a7c83adcb",
      response: "b41e6675-a329-40e0-aa01-44d2f444babe",
    };
    const writes = [];
    let pendingCommand = [];

    function writeFixedText(bytes, offset, length, text) {
      const encoded = new TextEncoder().encode(text).slice(0, length);
      bytes.set(encoded, offset);
    }

    function commandResponse(id, counter, command) {
      if (id === 0x10) {
        const response = new Uint8Array(18);
        response.set([0x58, response.length, 0, 0, id, counter]);
        response[8] = 2;
        response[9] = 7;
        response[16] = 83;
        response[17] = 1;
        return response;
      }
      if (id === 0x55) {
        const response = new Uint8Array(158);
        response.set([0x58, response.length, 0, 0, id, counter]);
        writeFixedText(response, 6, 16, "GDX-RB");
        writeFixedText(response, 38, 32, "GDX-RB TEST");
        writeFixedText(response, 94, 64, "Go Direct Respiration Belt");
        return response;
      }
      if (id === 0x51) {
        return Uint8Array.from([0x58, 10, 0, 0, id, counter, 0x02, 0, 0, 0]);
      }
      if (id === 0x50) {
        const response = new Uint8Array(154);
        response.set([0x58, response.length, 0, 0, id, counter]);
        response[6] = command[5];
        const view = new DataView(response.buffer);
        view.setUint32(8, 1, true);
        response[12] = 0;
        response[13] = 0;
        writeFixedText(response, 14, 60, "Force");
        writeFixedText(response, 74, 32, "N");
        view.setFloat64(106, 0.01, true);
        view.setFloat64(114, -50, true);
        view.setFloat64(122, 50, true);
        view.setUint32(130, 50000, true);
        view.setBigUint64(134, 1000000n, true);
        view.setUint32(142, 100000, true);
        view.setUint32(146, 1000, true);
        return response;
      }
      return Uint8Array.from([0x58, 6, 0, 0, id, counter]);
    }

    class FakeCharacteristic extends EventTarget {
      constructor(uuid) {
        super();
        this.uuid = uuid;
        this.value = new DataView(new ArrayBuffer(0));
      }
      async startNotifications() { return this; }
      async stopNotifications() { return this; }
      async writeValueWithResponse(value) {
        const bytes = Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
        writes.push(bytes);
        pendingCommand.push(...bytes);
        while (pendingCommand.length >= 2 && pendingCommand.length >= pendingCommand[1]) {
          const command = pendingCommand.splice(0, pendingCommand[1]);
          const id = command[4];
          const counter = command[2];
          const response = commandResponse(id, counter, command);
          for (let offset = 0; offset < response.length; offset += 20) {
            characteristics.response.emit(response.slice(offset, offset + 20));
          }
        }
      }
      async writeValue(value) { return this.writeValueWithResponse(value); }
      emit(bytes) {
        const value = Uint8Array.from(bytes);
        this.value = new DataView(value.buffer);
        this.dispatchEvent(new Event("characteristicvaluechanged"));
      }
    }

    const characteristics = {
      command: new FakeCharacteristic(uuids.command),
      response: new FakeCharacteristic(uuids.response),
    };
    const server = {
      connected: false,
      async getPrimaryService(uuid) {
        if (String(uuid).toLowerCase() !== uuids.service) throw new DOMException("Service unavailable", "NotFoundError");
        return {
          async getCharacteristic(characteristicUuid) {
            const normalized = String(characteristicUuid).toLowerCase();
            if (normalized === uuids.command) return characteristics.command;
            if (normalized === uuids.response) return characteristics.response;
            throw new DOMException("Characteristic unavailable", "NotFoundError");
          },
        };
      },
      disconnect() {
        this.connected = false;
        device.dispatchEvent(new Event("gattserverdisconnected"));
      },
    };
    const device = new EventTarget();
    device.name = "GDX-RB TEST";
    device.id = "fake-vernier-gdx";
    device.gatt = {
      get connected() { return server.connected; },
      async connect() {
        server.connected = true;
        return server;
      },
      disconnect() { server.disconnect(); },
    };
    let activationAtRequest = null;
    Object.defineProperty(navigator, "bluetooth", {
      configurable: true,
      value: {
        async requestDevice(options) {
          activationAtRequest = navigator.userActivation?.isActive ?? null;
          window.__vernierFake.lastRequest = options;
          return device;
        },
      },
    });
    window.__vernierFake = {
      writes,
      lastRequest: null,
      get activationAtRequest() { return activationAtRequest; },
      emitNormal(values) {
        const payload = new Uint8Array(9 + values.length * 4);
        payload.set([0x20, payload.length, 0, 0, 0x06, 0x02, 0, values.length, 0]);
        const view = new DataView(payload.buffer);
        values.forEach((value, index) => view.setFloat32(9 + index * 4, value, true));
        characteristics.response.emit(payload.slice(0, 7));
        characteristics.response.emit(payload.slice(7));
      },
    };
  });
}

await mkdir(output, { recursive: true });
const manifest = JSON.parse(await readFile(join(root, "browser-demo-manifest.json"), "utf8"));
assert.equal(manifest.canonicalSource, "apps/polar-stream/ui");
for (const [name, expected] of Object.entries(manifest.sha256)) {
  const canonical = await readFile(join(repository, manifest.canonicalSource, name));
  const staged = await readFile(join(root, name));
  assert.equal(sha256(canonicalAssetBytes(canonical, name)), expected, `${name} canonical hash differs from the manifest`);
  assert.equal(sha256(staged), expected, `${name} Pages artifact differs from the canonical UI`);
}

const server = await startServer();
const address = server.address();
const baseUrl = `http://127.0.0.1:${address.port}/`;
const browser = await chromium.launch({ headless: true });

try {
  const desktop = await browser.newPage({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    locale: "en-US",
  });
  await desktop.goto(baseUrl, { waitUntil: "networkidle" });
  assert.equal(await desktop.locator("body").getAttribute("data-runtime"), "browser-demo");
  assert.equal(await desktop.locator("#platform-label").textContent(), "BROWSER DEMO");
  assert.equal(await desktop.locator("#runtime-path-label").textContent(), "Browser-local inputs");
  assert.match(await desktop.locator(".device-row.mock").textContent(), /RECORDED/);
  assert.match(await desktop.locator(".device-row.mock").textContent(), /seamless loop of an anonymized 60-second ECG \+ ACC recording/i);
  assert.equal(await desktop.locator("body").getAttribute("data-device-profile"), "none");
  assert.equal(await desktop.locator("#output-empty-state").isVisible(), true, "Output must begin empty before a device connects");
  assert.equal(await desktop.locator("#output-workspace").isHidden(), true);
  assert.equal(await desktop.locator("#visual-empty-state").isVisible(), true, "Visualization must begin empty before a device connects");
  assert.equal(await desktop.locator("#visual-workspace").isHidden(), true);
  assert.equal(await desktop.locator("#output-state").textContent(), "Waiting");
  assert.equal(await desktop.locator("#visual-source option").count(), 0);
  await connectMock(desktop);
  assert.equal(await desktop.locator("body").getAttribute("data-device-profile"), "polar");
  assert.equal(await desktop.locator("#output-workspace").isVisible(), true);
  assert.equal(await desktop.locator("#visual-workspace").isVisible(), true);
  assert.deepEqual(
    await desktop.locator("#visual-source option").evaluateAll((options) => options.map((option) => option.value)),
    ["raw_ecg", "raw_acc"],
  );
  assert.equal(await desktop.locator("#browser-local-destination").isVisible(), true, "browser-local destination is missing");
  assert.equal(await desktop.locator("#csv-destination-row").isVisible(), true, "local CSV toggle is missing");
  assert.equal(await desktop.locator("#audio-destination-row").isVisible(), true, "audio-data toggle is missing");
  assert.equal(await desktop.locator("#lsl-destination-row").isVisible(), true, "shared LSL control is missing in browser mode");
  assert.equal(await desktop.locator("#osc-destination-row").isVisible(), true, "shared OSC control is missing in browser mode");
  const destinationLayout = await desktop.locator(".destination-options").evaluate((group) => {
    const rows = [...group.querySelectorAll(".destination-row")].map((row) => {
      const bounds = row.getBoundingClientRect();
      return { left: Math.round(bounds.left), top: Math.round(bounds.top), height: bounds.height };
    });
    return {
      columnCount: new Set(rows.map((row) => row.left)).size,
      rowCount: new Set(rows.map((row) => row.top)).size,
      heights: rows.map((row) => row.height),
      height: group.getBoundingClientRect().height,
    };
  });
  assert.equal(destinationLayout.columnCount, 2, `desktop destinations are not arranged in two columns: ${JSON.stringify(destinationLayout)}`);
  assert.equal(destinationLayout.rowCount, 2, `desktop destinations are not arranged in two rows: ${JSON.stringify(destinationLayout)}`);
  assert.ok(destinationLayout.heights.every((height) => height >= 44 && height <= 52), `desktop destination controls are not compact: ${JSON.stringify(destinationLayout)}`);
  assert.ok(destinationLayout.height <= 110, `desktop destination group is too tall: ${JSON.stringify(destinationLayout)}`);
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
  const metricTimestampCsv = await desktop.evaluate(async () => {
    let now = 2_000;
    const recorder = window.PolarBrowserSession.createRecorder({ maxRows: 10, now: () => now });
    recorder.configure({ streamName: "Metric timestamp test", outputs: ["breathing_volume"] });
    recorder.start({ deviceName: "Fixture", inputKind: "mock" });
    now = 2_100;
    recorder.capture({
      kind: "metrics",
      sensorTimestampNs: "3000000000",
      values: [{ id: "breathing_volume", value: 0.75 }],
    }, now);
    return recorder.createBlob().text();
  });
  assert.match(metricTimestampCsv, /,3000000000,breathing_volume,0,,,,0\.75,/);
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
  const python = process.env.PYTHON || (process.platform === "win32" ? "python" : "python3");
  const decoder = spawnSync(python, [
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
  assert.equal(await desktop.locator(".metric-preview-large animateTransform").count(), 1);
  assert.equal(await desktop.locator(".metric-scientific-summary").count(), 1);
  const rmssdSummary = await desktop.locator(".metric-scientific-summary p").textContent();
  const rmssdSentenceCount = (rmssdSummary.match(/[.!?](?:\s|$)/g) || []).length;
  assert.ok(rmssdSentenceCount >= 2 && rmssdSentenceCount <= 3);
  assert.ok(await desktop.locator(".metric-source-list a").count() >= 2);
  assert.equal(await desktop.locator("#metric-detail article > section").count(), 3);
  assert.equal(await desktop.locator(".metric-preview-settings, .metric-formula-context, .metric-stream-preview, .breathing-selection-settings").count(), 0);
  await desktop.locator("#open-formula-lab").click();
  assert.equal(await desktop.locator("#formula-dialog").isVisible(), true);
  await desktop.locator("#formula-name").fill("rmssd_custom");
  await desktop.locator("#formula-source").selectOption("rrInterval");
  await desktop.locator("#formula-unit").fill("ms");
  await desktop.locator("#formula-expression").fill("rr_rmssd(rr, 300)");
  assert.match(await desktop.locator(".formula-variable-map").textContent(), /Time is retained automatically|time is retained automatically/i);
  assert.equal(await desktop.locator('#formula-keyboard button[title]').count() > 10, true, "formula calculator keys lack hover help");
  await desktop.locator("#formula-validation-status").filter({ hasText: "Valid" }).waitFor();
  assert.notEqual(await desktop.locator("#formula-preview-current").textContent(), "—");
  assert.equal(await desktop.locator("#formula-preview-canvas").getAttribute("data-looping"), "true", "Formula Lab preview is not continuously looped");
  await desktop.locator("#save-custom-formula").click();
  await desktop.locator(".formula-output-card").filter({ hasText: "rmssd_custom" }).waitFor();
  await desktop.locator("#open-output-dialog").click();
  await desktop.getByRole("button", { name: /ACC metrics/ }).click();
  await desktop.locator('.metric-option[data-metric-id="breathing_volume"]').click();
  await desktop.locator("#save-metric-output").click();
  await desktop.locator("#visual-source").selectOption("breathing_volume");
  await desktop.waitForFunction(() => Number(document.querySelector("#signal-canvas")?.dataset.trailPoints) > 10);
  assert.match(await desktop.locator("#chart-shell").getAttribute("class"), /breathing-trail-visual/);
  assert.match(await desktop.locator("#signal-canvas").getAttribute("aria-label"), /moving dot.*leftward trail/i);
  assert.match(await desktop.locator("#signal-canvas").getAttribute("data-breath-direction"), /inhale|exhale|pause/);
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

  const bluetooth = await browser.newPage({
    viewport: { width: 1280, height: 820 },
    deviceScaleFactor: 1,
    locale: "en-US",
  });
  await installFakeWebBluetooth(bluetooth);
  await bluetooth.goto(baseUrl, { waitUntil: "networkidle" });
  const webBluetoothRow = bluetooth.locator('.device-row[data-input-kind="web-bluetooth"]');
  assert.match(await webBluetoothRow.textContent(), /EXPERIMENTAL/);
  assert.match(await webBluetoothRow.textContent(), /Connect/);
  assert.equal(await bluetooth.locator("#scan-button span").textContent(), "Search devices");
  await bluetooth.locator("#scan-button").click();
  await bluetooth.waitForFunction(() => document.querySelector("#scan-button span")?.textContent === "Search devices");
  assert.equal(await bluetooth.evaluate(() => window.__polarFake.lastRequest ?? null), null, "Search opened a Bluetooth chooser before Connect");
  await bluetooth.evaluate(() => window.__polarFake.cancelNextChooser());
  await webBluetoothRow.click();
  await bluetooth.locator("#input-state").filter({ hasText: "Browser ready" }).waitFor();
  assert.equal(await bluetooth.locator("#app-state-text").textContent(), "Browser inputs ready");
  assert.match(await bluetooth.locator("#connection-detail").textContent(), /No sensor was selected/);
  assert.equal(await bluetooth.locator(".toast.error").count(), 0, "chooser cancellation must not create an error toast");
  assert.equal(await bluetooth.locator("#scan-button span").textContent(), "Search devices");
  await bluetooth.evaluate(() => window.__polarFake.disableNextChooser());
  await webBluetoothRow.click();
  await bluetooth.locator("#input-state").filter({ hasText: "Error" }).waitFor();
  assert.equal(await bluetooth.locator("#app-state-text").textContent(), "Connection failed");
  assert.match(await bluetooth.locator("#connection-detail").textContent(), /browser blocks Web Bluetooth/i);
  assert.equal(await bluetooth.locator(".toast.error").count(), 1, "a browser-level Bluetooth block must be visible as an error");
  await bluetooth.waitForFunction(() => !document.querySelector(".toast"));
  await bluetooth.evaluate(() => window.__polarFake.blockNextChooserWithPolicy());
  await webBluetoothRow.click();
  await bluetooth.locator("#input-state").filter({ hasText: "Error" }).waitFor();
  assert.match(await bluetooth.locator("#connection-detail").textContent(), /embedding policy blocks Web Bluetooth/i);
  await bluetooth.waitForFunction(() => !document.querySelector(".toast"));
  await bluetooth.evaluate(() => {
    window.__polarFake.failNextGattConnect();
    window.__polarFake.useLegacyControlWrites();
  });
  await webBluetoothRow.click();
  try {
    await bluetooth.locator("#input-state").filter({ hasText: "Browser BLE live" }).waitFor();
  } catch (error) {
    const diagnostic = await bluetooth.evaluate(() => ({
      inputState: document.querySelector("#input-state")?.textContent,
      appState: document.querySelector("#app-state-text")?.textContent,
      detail: document.querySelector("#connection-detail")?.textContent,
      toast: document.querySelector(".toast")?.textContent,
      writes: window.__polarFake?.writes,
      gattConnectAttempts: window.__polarFake?.gattConnectAttempts,
    }));
    throw new Error(`Polar Web Bluetooth fixture did not connect: ${JSON.stringify(diagnostic)}`, { cause: error });
  }
  assert.equal(await bluetooth.locator('.device-row[data-input-kind="web-bluetooth"]').count(), 0, "connected H10 remained a discovery row");
  assert.equal(await bluetooth.locator('.connected-device-widget[data-device-profile="polar"]').count(), 1, "connected H10 did not become a widget");
  assert.equal(await bluetooth.getByLabel(/Color for Polar H10 TEST1234/).count(), 1, "connected H10 widget has no color picker");
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
    const noisyProcessor = window.PolarWebBluetooth.createBreathingProcessor({
      calibrationWindowSeconds: 1,
      minimumAxisRangeG: 0.001,
      smoothingWindowSeconds: 0.75,
    });
    let noisySnapshot = null;
    for (let block = 0; block < 125; block += 1) {
      const samples = Array.from({ length: 20 }, (_, offset) => {
        const index = block * 20 + offset;
        const noise = index % 2 === 0 ? 3 : -3;
        return {
          xMg: noise,
          yMg: -noise,
          zMg: 1000 + noise + Math.round(30 * Math.sin(index / 200 * Math.PI * 2 * 0.25)),
        };
      });
      noisySnapshot = noisyProcessor.push(samples, block * 100);
    }
    const threshold = processor.phaseVelocityThresholdPerSecond();
    const equivalentBatchPhases = [
      processor.classifyPhaseDelta(0.004, 0.05),
      processor.classifyPhaseDelta(0.012, 0.15),
      processor.classifyPhaseDelta(-0.004, 0.05),
      processor.classifyPhaseDelta(-0.012, 0.15),
      processor.classifyPhaseDelta(0.002, 0.05),
      processor.classifyPhaseDelta(0.006, 0.15),
    ];
    return {
      decoded, compressed, malformedCode, snapshot, noisySnapshot, threshold, equivalentBatchPhases,
    };
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
  assert.equal(protocolChecks.snapshot.ready, true, "browser breathing waveform did not become ready");
  assert.ok(protocolChecks.snapshot.confidence01 > 0, "browser breathing confidence did not become positive");
  assert.ok(protocolChecks.snapshot.axisRangeG >= 0.001, "browser breathing range is too small");
  assert.equal(protocolChecks.noisySnapshot.ready, true, "ordinary three-axis sensor noise blocked browser readiness");
  assert.ok(protocolChecks.noisySnapshot.confidence01 > 0.20, "noisy clean browser waveform lost confidence");
  assert.ok(Math.abs(protocolChecks.threshold - 0.06) < 1e-6, "browser phase velocity threshold drifted from Rust");
  assert.deepEqual(protocolChecks.equivalentBatchPhases, [1, 1, -1, -1, 0, 0]);
  await bluetooth.locator("#open-output-dialog").click();
  await bluetooth.locator('.metric-option[data-metric-id="ecg_mean"]').click();
  assert.equal(await bluetooth.locator("#save-metric-output").textContent(), "Desktop only");
  assert.match(await bluetooth.locator("#dialog-output-status").textContent(), /requires the desktop app/);
  await assertNoHorizontalOverflow(bluetooth, "desktop Web Bluetooth input");
  await bluetooth.close();

  const vernier = await browser.newPage({
    viewport: { width: 1280, height: 820 },
    deviceScaleFactor: 1,
    locale: "en-US",
  });
  await installFakeVernierWebBluetooth(vernier);
  await vernier.goto(baseUrl, { waitUntil: "networkidle" });
  assert.equal(await vernier.locator("body").getAttribute("data-device-profile"), "none");
  assert.equal(await vernier.locator("#output-empty-state").isVisible(), true);
  assert.equal(await vernier.locator("#visual-empty-state").isVisible(), true);
  const vernierRow = vernier.locator('.device-row[data-input-kind="web-bluetooth-vernier"]');
  assert.match(await vernierRow.textContent(), /Vernier Go Direct via browser/);
  assert.match(await vernierRow.textContent(), /Connect/);
  await vernierRow.click();
  await vernier.waitForFunction(() => window.VernierWebBluetooth.activeSources().length === 1);
  const vernierContract = await vernier.evaluate(() => ({
    request: window.__vernierFake.lastRequest,
    activationAtRequest: window.__vernierFake.activationAtRequest,
    writes: window.__vernierFake.writes,
  }));
  assert.deepEqual(vernierContract.request.filters, [{ namePrefix: "GDX" }]);
  assert.deepEqual(vernierContract.request.optionalServices, ["d91714ef-28b9-4f91-ba16-f0d9a604f112"]);
  assert.equal(vernierContract.activationAtRequest, true, "Go Direct chooser lost its initiating user activation");
  assert.ok(vernierContract.writes.length >= 8, "Go Direct setup did not send the complete initialization sequence");
  assert.equal(vernierContract.writes[0][0], 0x58);
  assert.equal(vernierContract.writes[0][4], 0x1a);
  await vernier.evaluate(() => window.__vernierFake.emitNormal([1.25, -2.5, 3.75]));
  await vernier.locator("#input-state").filter({ hasText: "Browser BLE live" }).waitFor();
  assert.equal(await vernier.locator('.device-row[data-input-kind="web-bluetooth-vernier"]').count(), 0, "connected GDX remained a discovery row");
  assert.equal(await vernier.locator('.connected-device-widget[data-device-profile="vernier"]').count(), 1, "connected GDX did not become a widget");
  await vernier.waitForFunction(() => document.querySelector("#raw-force-value")?.textContent === "3.750");
  assert.equal(await vernier.locator("#connection-metric-1-label").textContent(), "FORCE");
  assert.equal(await vernier.locator("#connection-metric-1-value").textContent(), "10 Hz");
  assert.equal(await vernier.locator("#connection-metric-2-value").textContent(), "1");
  assert.match(await vernier.locator("#connection-detail").textContent(), /Browser source 1/);
  assert.match(await vernier.locator("#connection-detail").textContent(), /firmware 2\.7/);
  assert.equal(await vernier.locator("#battery-value").textContent(), "83%");
  assert.match(await vernier.locator("#app-state-text").textContent(), /Go Direct connected directly/);
  assert.match(await vernier.locator(".active-source-chip").textContent(), /Browser source 1.*GDX-RB TEST/);
  assert.equal(await vernier.locator("#chart-shell").evaluate((node) => node.style.getPropertyValue("--source-color")), "#00c2ff");
  assert.equal(await vernier.locator("body").getAttribute("data-device-profile"), "vernier");
  assert.match(await vernier.locator("#device-profile-title").textContent(), /respiration belt/i);
  assert.match(await vernier.locator("#device-profile-description").textContent(), /Primary use: breathing/i);
  assert.equal(await vernier.locator("#raw-ecg-card").isHidden(), true);
  assert.equal(await vernier.locator("#raw-acc-card").isHidden(), true);
  assert.equal(await vernier.locator("#raw-force-card").isVisible(), true);
  assert.equal(await vernier.locator("#vernier-breathing-card").isVisible(), true);
  assert.equal(await vernier.locator("#output-chips > *").count(), 0, "fresh Vernier connections must not add a redundant raw-force compatibility stream");
  assert.equal(await vernier.locator("#output-state").textContent(), "2 signals");
  assert.match(await vernier.locator("#vernier-breathing-value").textContent(), /^(?:0\.\d{3}|1\.000)$/);
  assert.deepEqual(
    await vernier.locator("#visual-source option").evaluateAll((options) => options.map((option) => option.value)),
    ["raw_force", "vernier_breathing"],
  );
  assert.equal(await vernier.locator("#visual-source").inputValue(), "vernier_breathing");
  await vernier.locator("#open-output-dialog").click();
  await vernier.locator('.metric-option[data-metric-id="raw_force"]').waitFor();
  assert.equal(await vernier.locator("#output-dialog").getAttribute("data-family"), "vernier");
  assert.equal(await vernier.locator("#metric-family-toggle").isHidden(), true);
  assert.equal(await vernier.locator("#vernier-protocol-panel").isVisible(), true);
  assert.equal(await vernier.locator("#vernier-protocol-panel article").count(), 2);
  assert.deepEqual(
    await vernier.locator(".metric-option").evaluateAll((options) => options.map((option) => option.dataset.metricId)),
    ["raw_force"],
  );
  assert.equal(await vernier.locator("#open-formula-lab").isHidden(), true);
  assert.match(await vernier.locator("#vernier-protocol-note").textContent(), /require the installed app/i);
  await vernier.locator('#output-dialog [aria-label="Close"]').click();
  await assertNoHorizontalOverflow(vernier, "desktop Go Direct Web Bluetooth input");
  await vernier.locator('.connected-device-widget[data-device-profile="vernier"] .connected-device-disconnect').click();
  await vernier.locator("#input-state").filter({ hasText: "Browser ready" }).waitFor();
  assert.equal(await vernier.locator("body").getAttribute("data-device-profile"), "none");
  assert.equal(await vernier.locator("#output-empty-state").isVisible(), true);
  assert.equal(await vernier.locator("#output-workspace").isHidden(), true);
  assert.equal(await vernier.locator("#visual-empty-state").isVisible(), true);
  assert.equal(await vernier.locator("#visual-workspace").isHidden(), true);
  await vernier.close();

  const phone = await browser.newPage({
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 1,
    locale: "en-US",
    hasTouch: true,
    isMobile: true,
    userAgent: "Mozilla/5.0 (Linux; Android 14; Moto G) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36",
  });
  await installFakeWebBluetooth(phone);
  await phone.goto(baseUrl, { waitUntil: "networkidle" });
  await assertNoHorizontalOverflow(phone, "phone browser demo before connection");
  assert.equal(await phone.locator("#output-empty-state").isVisible(), true);
  assert.equal(await phone.locator("#visual-empty-state").isVisible(), true);
  const panelTops = await phone.locator(".workspace-panel").evaluateAll((panels) => panels.map((panel) => panel.getBoundingClientRect().top));
  assert.ok(panelTops[0] < panelTops[1] && panelTops[1] < panelTops[2], `phone panels are not stacked: ${panelTops}`);
  const scanBox = await phone.locator("#scan-button").boundingBox();
  assert.ok(scanBox.height >= 44, `phone primary action is too short: ${scanBox.height}`);
  await phone.locator("#scan-button").click();
  await phone.waitForFunction(() => document.querySelector("#scan-button span")?.textContent === "Search devices");
  assert.equal(await phone.evaluate(() => window.__polarFake.lastRequest ?? null), null, "phone Search opened a chooser before Connect");
  await phone.evaluate(() => window.__polarFake.disableNextChooser());
  await phone.locator('.device-row[data-input-kind="web-bluetooth"]').click();
  await phone.locator("#input-state").filter({ hasText: "Error" }).waitFor();
  assert.match(await phone.locator("#connection-detail").textContent(), /Google Chrome on Android/);
  await phone.waitForFunction(() => !document.querySelector(".toast"));
  await phone.locator('.device-row[data-input-kind="web-bluetooth"]').click();
  await phone.locator("#input-state").filter({ hasText: "Browser BLE live" }).waitFor();
  assert.deepEqual(await phone.evaluate(() => window.__polarFake.wakeLockRequests), ["screen"]);
  await phone.locator('.connected-device-widget[data-device-profile="polar"] .connected-device-disconnect').click();
  await phone.locator("#input-state").filter({ hasText: "Browser ready" }).waitFor();
  await connectMock(phone);
  assert.equal(await phone.locator("#lsl-destination-row").isVisible(), true, "phone browser UI hid the shared LSL control after connection");
  assert.equal(await phone.locator("#osc-destination-row").isVisible(), true, "phone browser UI hid the shared OSC control after connection");
  await phone.locator("#lsl-destination-row").click();
  assert.equal(await phone.locator("#lsl-toggle").isChecked(), false, "phone browser LSL must fail closed");
  assert.equal(await phone.locator("#native-output-browser-error").isVisible(), true, "phone browser LSL refusal is not visible");
  const downloadLinkBox = await phone.locator("#native-output-browser-error a").boundingBox();
  assert.ok(downloadLinkBox.height >= 44, `phone installed-app download link is too short: ${downloadLinkBox.height}`);
  await phone.locator(".toast").evaluateAll((toasts) => toasts.forEach((toast) => toast.remove()));

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
  await phone.locator('.metric-option[data-metric-id="breathing_volume"]').click();
  await phone.locator("#save-metric-output").click();
  await phone.locator("#visual-source").selectOption("breathing_volume");
  await phone.waitForFunction(() => Number(document.querySelector("#signal-canvas")?.dataset.trailPoints) > 10);
  assert.match(await phone.locator("#chart-shell").getAttribute("class"), /breathing-trail-visual/);
  assert.match(await phone.locator("#signal-canvas").getAttribute("aria-label"), /moving dot.*leftward trail/i);
  await assertNoHorizontalOverflow(phone, "phone breathing trail");
  await phone.locator("#open-output-dialog").click();
  await phone.getByRole("button", { name: /ACC metrics/ }).click();
  await phone.locator('.metric-option[data-metric-id="breathing_phase"]').click();
  assert.equal(await phone.locator("#output-dialog").getAttribute("data-mobile-view"), "detail");
  assert.equal(await phone.locator("#metric-options").isHidden(), true, "phone detail view must hide the browse list");
  assert.equal(await phone.locator("#metric-detail").isVisible(), true, "phone detail view did not open");
  assert.equal(await phone.getByRole("button", { name: "Back to all signals" }).isVisible(), true);
  assert.equal(await phone.locator(".metric-preview-large animateTransform").count(), 1);
  assert.equal(await phone.locator(".metric-scientific-summary").count(), 1);
  assert.ok(await phone.locator(".metric-source-list a").count() >= 2);
  assert.equal(await phone.locator("#metric-detail article > section").count(), 3);
  assert.equal(await phone.locator(".metric-preview-settings, .metric-formula-context, .metric-stream-preview, .breathing-selection-settings").count(), 0);
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

  const narrow = await browser.newPage({
    viewport: { width: 320, height: 720 },
    hasTouch: true,
    isMobile: true,
    locale: "en-US",
  });
  await narrow.goto(baseUrl, { waitUntil: "networkidle" });
  await assertNoHorizontalOverflow(narrow, "320px browser demo");
  assert.ok(await narrow.locator(".device-row.mock").isVisible(), "mock input is not visible at 320px");
  await narrow.close();

  process.stdout.write(`Validated canonical Pages parity, recorded H10 replay, Formula Lab, Polar PMD + Vernier Go Direct Web Bluetooth, and responsive layouts in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
