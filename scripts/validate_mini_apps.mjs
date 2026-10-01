import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { chromium } from "playwright";

const root = process.cwd();
const outputDirectory = path.join(root, "target", "ui-qa");
await fs.mkdir(outputDirectory, { recursive: true });

const apps = [
  {
    kind: "polar",
    productName: "Polar Stream Mini",
    scanLabel: "Polar H10",
    streamName: "Polar-H10-Mini",
    height: 332,
    frameColors: ["rgb(255, 255, 255)", "rgb(0, 0, 0)", "rgb(213, 0, 28)"],
  },
  {
    kind: "vernier",
    productName: "Vernier Stream Mini",
    scanLabel: "Vernier Go Direct",
    streamName: "Vernier-GDX-Mini",
    height: 354,
    frameColors: ["rgb(255, 255, 255)", "rgb(245, 154, 47)", "rgb(0, 124, 122)"],
  },
];

const browser = await chromium.launch({ headless: true, args: ["--allow-file-access-from-files"] });
try {
  for (const app of apps) {
    await validateNormalWindow(app);
    await validateEarlyRememberedConnection(app);
    if (app.kind === "vernier") {
      await validateBluetoothUnavailable(app);
      await validateBluetoothOff(app);
    }
    await validateMockWindow(app, true);
    await validateMockWindow(app, false);
  }
} finally {
  await browser.close();
}

async function validateBluetoothUnavailable(app) {
  const page = await createPage(app, { mockMode: false, lslHealthy: false, scanError: true });
  await page.goto(appUrl(app));
  await page.locator("#scan-button").click();
  await page.locator("#device-scan-status").filter({ hasText: "Bluetooth is unavailable." }).waitFor();
  assert.match(await page.locator("#device-scan-status").textContent(), /Check Windows Bluetooth settings/);
  assert.equal(await page.locator("#device-error-details").isVisible(), true);
  await page.locator("#device-rescan").waitFor({ state: "visible" });
  await assertNoOverflow(page);
  await page.close();
}

async function validateBluetoothOff(app) {
  const page = await createPage(app, { mockMode: false, lslHealthy: true, radioState: "off" });
  await page.goto(appUrl(app));
  await page.locator("#scan-button").click();
  await page.locator("#device-scan-status").filter({ hasText: "Bluetooth is off." }).waitFor();
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("scan_devices")), false);
  await page.locator("#bluetooth-toggle").check();
  await page.locator("#device-results .discovered-device").waitFor();
  assert.equal(await page.locator("#bluetooth-state").textContent(), "On");
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("set_bluetooth_radio")), true);
  await assertNoOverflow(page);
  await page.close();
}

async function validateEarlyRememberedConnection(app) {
  const page = await createPage(app, { mockMode: false, lslHealthy: true, remembered: true });
  await page.goto(appUrl(app));
  await page.locator("#mini-node.connected").waitFor();
  assert.equal(await page.locator("#node-phase").textContent(), app.kind === "vernier" ? "Connected" : "Live");
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("connect_remembered")), true);
  await page.close();
}

async function validateNormalWindow(app) {
  const page = await createPage(app, { mockMode: false, lslHealthy: true });
  await page.goto(appUrl(app));
  await page.locator("#node-phase").filter({ hasText: "Ready" }).waitFor();

  const frame = await frameState(page);
  assert.equal(frame.radius, "16px");
  assert.equal(frame.border, app.frameColors[0]);
  assert.ok(frame.shadow.includes(app.frameColors[1]));
  assert.ok(frame.shadow.includes(app.frameColors[2]));
  assert.equal(frame.animation, "none");
  assert.equal(await page.locator("#device-row").isVisible(), true);
  assert.equal(await page.locator("#mock-source").isVisible(), false);
  assert.equal(await page.getByRole("button", { name: "Open mock" }).isVisible(), true);
  await assertNoOverflow(page);

  if (app.kind === "vernier") {
    assert.equal(await page.locator("#connection-feedback").textContent(), "Ready");
    await page.locator('#connection-feedback[data-text-fit="fit"]').waitFor();
    await page.locator("#metrics-button").click();
    assert.equal(await page.locator("#metric-options input").count(), 4);
    assert.equal(await page.locator("#metric-count").textContent(), "4/4");
    await page.locator('#metric-options input[value="rawVernier"]').uncheck();
    await page.waitForFunction(() => window.__miniSaves.some((save) => save.vernierOutputs?.length === 3));
    assert.equal(await page.locator("#metric-count").textContent(), "3/4");
    assert.doesNotMatch(await page.locator("#signal-list").textContent(), /rawVernier/);
    assert.equal(await page.evaluate(() => window.__miniCalls.includes("disconnect_device")), false);
    const savedPreferences = await page.evaluate(() => window.__miniSaves.at(-1));
    const reopened = await createPage(app, { mockMode: false, lslHealthy: true, savedPreferences });
    await reopened.goto(appUrl(app));
    await reopened.locator("#metrics-button").click();
    assert.equal(await reopened.locator('#metric-options input[value="rawVernier"]').isChecked(), false);
    await reopened.setViewportSize({ width: 320, height: 560 });
    await assertNoOverflow(reopened);
    await reopened.screenshot({ path: path.join(outputDirectory, "vernier-mini-outputs.png"), omitBackground: true });
    await reopened.locator('#metric-options input[value="rawForce"]').uncheck();
    await reopened.locator('#metric-options input[value="vernierBreathing"]').uncheck();
    await reopened.locator('#metric-options input[value="signalStatus"]').click();
    assert.equal(await reopened.locator('#metric-options input[value="signalStatus"]').isChecked(), true);
    assert.equal(await reopened.locator("#metric-count").textContent(), "1/4");
    await reopened.close();
    await page.locator("#metrics-close").click();
    assert.equal(await page.locator("#device-select option").count(), 1);
    await page.locator("#scan-button").click();
    await page.locator("#device-results .discovered-device").waitFor();
    assert.match(await page.locator("#device-scan-status").textContent(), /1 Go Direct device found/);
    await page.waitForFunction(() => getComputedStyle(document.querySelector(".bluetooth-track span")).transform.endsWith("16, 0)"));
    await page.screenshot({ path: path.join(outputDirectory, "vernier-mini-device-discovery.png"), omitBackground: true });
    await page.locator("#device-results .discovered-device").click();
    await page.waitForFunction(() => window.__miniCalls.includes("connect_device"));
    assert.equal(await page.locator("#device-select option").count(), 2);
    assert.match(await page.locator("#device-select option").nth(1).textContent(), /GDX-RB/);
    assert.equal(await page.locator("#connection-feedback").textContent(), "Connected; waiting for fresh LSL samples");
    await page.evaluate(() => window.__emitMiniEvent({ kind: "samples", vernierRows: 20, metricSamples: 20, lsl: "Publishing 3 LSL streams" }));
    assert.equal(await page.locator("#connection-feedback").textContent(), "Live: sensor samples reaching LSL");
    await page.evaluate(() => window.__emitMiniEvent({ kind: "connection", connected: false, message: "Disconnected" }));
  }

  if (app.kind === "polar") {
    const accIds = [
      "acc_breathing_magnitude", "breathing_phase", "breathing_rate",
      "breath_interval_mean", "phan_breath_event", "phan_breath_rate",
      "flowborne_phase", "flowborne_motion_score",
    ];
    await page.locator("#metrics-button").click();
    for (const id of accIds) {
      const option = page.locator(`#metric-options input[value="${id}"]`);
      assert.equal(await option.count(), 1);
      assert.match(await option.locator("xpath=../../..").textContent(), /ACC derived/);
      await option.check();
    }
    await page.waitForFunction(() => window.__miniSaves.some((save) =>
      ["acc_breathing_magnitude", "breathing_phase", "breathing_rate", "breath_interval_mean", "phan_breath_event", "phan_breath_rate", "flowborne_phase", "flowborne_motion_score"].every((id) => save.polarOutputs?.includes(id))));
    const savedPreferences = await page.evaluate(() => window.__miniSaves.at(-1));
    const reopened = await createPage(app, { mockMode: false, lslHealthy: true, savedPreferences });
    await reopened.goto(appUrl(app));
    await reopened.locator("#metrics-button").click();
    for (const id of accIds) {
      assert.equal(await reopened.locator(`#metric-options input[value="${id}"]`).isChecked(), true);
    }
    await reopened.setViewportSize({ width: 320, height: 560 });
    await assertNoOverflow(reopened);
    await reopened.screenshot({
      path: path.join(outputDirectory, "polar-mini-metrics-reset.png"),
      omitBackground: true,
    });
    await reopened.locator("#reset-metrics").click();
    await reopened.waitForFunction(() => window.__miniSaves.some((save) =>
      save.polarOutputs?.length === 4 && !save.polarOutputs.includes("phan_breath_event")));
    assert.equal(await reopened.locator("#reset-metrics").isDisabled(), true);
    assert.equal(await reopened.evaluate(() => window.__miniCalls.includes("disconnect_device")), false);
    const resetPreferences = await reopened.evaluate(() => window.__miniSaves.at(-1));
    await reopened.close();
    const resetReopened = await createPage(app, { mockMode: false, lslHealthy: true, savedPreferences: resetPreferences });
    await resetReopened.goto(appUrl(app));
    await resetReopened.locator("#metrics-button").click();
    assert.equal(await resetReopened.locator('#metric-options input[value="phan_breath_event"]').isChecked(), false);
    await resetReopened.close();
    await page.locator("#metrics-close").click();
  }

  await page.locator("#stream-mode-toggle").check();
  await page.waitForFunction(() => window.__miniSaves.some((save) => save.outputMode === "singleStream"));
  assert.equal(await page.locator("#stream-mode-toggle").isChecked(), true);
  assert.match(await page.locator("#signal-list").textContent(), /single sparse LSL/);
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("disconnect_device")), false);

  await page.evaluate(() => window.__emitMiniEvent({
    kind: "connection",
    connected: true,
    deviceName: "Connected sensor",
    message: "Connected",
  }));
  assert.equal(await page.locator("#mock-button").isEnabled(), true);
  await page.locator("#mock-button").click();
  await page.waitForFunction(() => window.__miniCalls.includes("open_mock_node"));
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("start_mock_stream")), false);
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("disconnect_device")), false);
  await page.screenshot({
    path: path.join(outputDirectory, `${app.kind}-stream-mini-mock-action.png`),
    omitBackground: true,
  });
  await page.setViewportSize({ width: 330, height: app.height });
  await assertNoOverflow(page);
  assert.equal(await page.locator("#mock-button").evaluate((button) => button.scrollWidth <= button.clientWidth), true);
  await page.close();
}

async function validateMockWindow(app, lslHealthy) {
  const page = await createPage(app, { mockMode: true, lslHealthy });
  await page.goto(appUrl(app));
  await page.locator("#product-name").filter({ hasText: "Mock" }).waitFor();
  await page.waitForFunction(() => Number(document.querySelector("#sample-status")?.textContent?.match(/\d+/)?.[0]) > 0);

  assert.equal(await page.locator("#device-row").isVisible(), false);
  assert.equal(await page.locator("#mock-source").isVisible(), true);
  assert.equal(await page.locator("#mock-button").isVisible(), false);
  assert.equal(await page.locator("#node-kind").textContent(), "MOCK");
  const frame = await frameState(page);
  assert.equal(frame.animation, lslHealthy ? "stream-beacon" : "none");
  assert.equal(await page.locator("#mini-node").evaluate((node) => node.classList.contains("streaming")), lslHealthy);
  await assertNoOverflow(page);

  await page.locator("#stream-mode-toggle").check();
  await page.waitForFunction(() => window.__miniSaves.some((save) => save.outputMode === "singleStream"));
  assert.equal(await page.locator("#stream-mode-toggle").isChecked(), true);
  assert.equal(await page.evaluate(() => window.__miniCalls.includes("disconnect_device")), false);

  if (lslHealthy) {
    await page.screenshot({
      path: path.join(outputDirectory, `${app.kind}-stream-mini-mock-live.png`),
      omitBackground: true,
    });
    await page.waitForTimeout(1500);
    assert.equal(await page.locator("#mini-node").evaluate((node) => node.classList.contains("streaming")), false);
    await page.evaluate(({ kind, lsl }) => window.__emitMiniEvent({
      kind: "samples",
      ecgSamples: kind === "polar" ? 78 : 0,
      accSamples: kind === "polar" ? 120 : 0,
      vernierRows: kind === "vernier" ? 12 : 0,
      metricSamples: kind === "vernier" ? 12 : 0,
      lsl,
    }), { kind: app.kind, lsl: "Publishing 1 LSL stream" });
    await page.waitForFunction(() => document.querySelector("#mini-node")?.classList.contains("streaming"));
  }
  await page.close();
}

async function createPage(app, options) {
  const page = await browser.newPage({ viewport: { width: 388, height: app.height } });
  await page.addInitScript(
    ({ app, options }) => {
      const calls = [];
      const saves = [];
      let eventChannel = null;
      let radioState = options.radioState || "on";
      window.__miniCalls = calls;
      window.__miniSaves = saves;
      window.__emitMiniEvent = (event) => eventChannel?.onmessage?.(event);

      class Channel {
        onmessage = null;
      }

      const preferences = {
        schema: "polar.stream.mini.preferences.v1",
        streamName: `${app.streamName}${options.mockMode ? "-Mock-1234" : ""}`,
        outputMode: "separateStreams",
        autoConnect: Boolean(options.remembered),
        polarOutputs: ["raw_ecg", "raw_acc", "heart_rate", "rr_interval"],
        vernierOutputs: ["rawVernier", "rawForce", "vernierBreathing", "signalStatus"],
        lastDevice: options.remembered ? { id: "saved-device", name: `Saved ${app.scanLabel}` } : null,
        ...options.savedPreferences,
      };
      const lsl = options.lslHealthy ? "Publishing 4 LSL streams" : "LSL unavailable";
      const session = {
        connected: true,
        deviceId: `mock://${app.kind}-1234`,
        deviceName: `Mock ${app.scanLabel}`,
        streamName: preferences.streamName,
        outputMode: preferences.outputMode,
        lsl,
      };

      async function invoke(command, payload = {}) {
        calls.push(command);
        if (command === "attach_events") {
          eventChannel = payload.events;
          return null;
        }
        if (command === "get_bootstrap") {
          return {
            kind: app.kind,
            productName: app.productName,
            scanLabel: app.scanLabel,
            preferences,
            metrics: app.kind === "polar" ? [
              { id: "acc_breathing_magnitude", streamSuffix: "accBreathingMagnitude", label: "ACC breathing projection", detail: "Signed ACC projection", category: "Breathing", direct: false },
              { id: "breathing_phase", streamSuffix: "breathingPhase", label: "Breathing phase", detail: "Three-state classifier", category: "Breathing", direct: false },
              { id: "breathing_rate", streamSuffix: "breathingRate", label: "Breathing rate", detail: "Cycle rate", category: "Breathing", direct: false },
              { id: "breath_interval_mean", streamSuffix: "breathIntervalMean", label: "Breath interval mean", detail: "Cycle interval", category: "Breathing dynamics", direct: false },
              { id: "phan_breath_event", streamSuffix: "phanBreathEvent", label: "Phan ACC breath event", detail: "Detected breath pulse", category: "Breathing", direct: false },
              { id: "phan_breath_rate", streamSuffix: "phanBreathRate", label: "Phan ACC breath count rate", detail: "Rolling count", category: "Breathing", direct: false },
              { id: "flowborne_phase", streamSuffix: "flowbornePhase", label: "Flowborne ACC phase", detail: "Four-state classifier", category: "Breathing", direct: false },
              { id: "flowborne_motion_score", streamSuffix: "flowborneMotionScore", label: "Flowborne ACC motion score", detail: "Signed contrast", category: "Breathing", direct: false },
            ] : [],
            session: null,
            mockMode: options.mockMode,
            lslResourcePresent: true,
          };
        }
        if (command === "get_autostart") return false;
        if (command === "get_bluetooth_radio") return { state: radioState };
        if (command === "set_bluetooth_radio") {
          radioState = payload.enabled ? "on" : "off";
          return { state: radioState };
        }
        if (command === "connect_remembered") {
          eventChannel?.onmessage?.({
            kind: "connection", connected: true, deviceName: `Saved ${app.scanLabel}`,
            message: "Streaming",
          });
          return { ...session, connected: false, deviceId: "saved-device", deviceName: null };
        }
        if (command === "open_mock_node" || command === "open_new_node") {
          return { launched: true, message: "Opened" };
        }
        if (command === "save_preferences") {
          saves.push(payload.preferences);
          return { preferences: { ...preferences, ...payload.preferences }, applied: true, reconnectRequired: false, message: "Saved and applied." };
        }
        if (command === "start_mock_stream") {
          window.setTimeout(() => {
            eventChannel?.onmessage?.({
              kind: "connection",
              connected: true,
              streaming: options.lslHealthy,
              deviceName: session.deviceName,
              modelCode: "MOCK",
              message: "Publishing synthetic data",
            });
          }, 0);
          window.setTimeout(() => {
            eventChannel?.onmessage?.({
              kind: "samples",
              ecgSamples: app.kind === "polar" ? 39 : 0,
              accSamples: app.kind === "polar" ? 60 : 0,
              heartRatePackets: app.kind === "polar" ? 1 : 0,
              metricSamples: app.kind === "vernier" ? 6 : 0,
              vernierRows: app.kind === "vernier" ? 6 : 0,
              droppedBatches: 0,
              queueHighWater: 0,
              lsl,
            });
          }, 40);
          return session;
        }
        if (command === "disconnect_device") return { ...session, connected: false, lsl: "Off" };
        if (command === "scan_devices") {
          if (options.scanError) throw { code: "BLUETOOTH_UNAVAILABLE", message: "No Bluetooth Low Energy adapter was found.", retryable: true };
          return app.kind === "vernier"
            ? [{ id: "gdx-rb-1", name: "GDX-RB 1234", modelCode: "GDX-RB", rssi: -52 }]
            : [];
        }
        if (command === "connect_device") {
          eventChannel?.onmessage?.({ kind: "connection", connected: true, deviceName: "GDX-RB 1234", message: "Publishing Vernier" });
          return { ...session, connected: true, deviceId: payload.deviceId, deviceName: "GDX-RB 1234" };
        }
        return null;
      }

      window.__TAURI__ = {
        core: { Channel, invoke },
        window: { getCurrentWindow: () => ({ minimize: async () => {} }) },
      };
    },
    { app, options },
  );
  return page;
}

function appUrl(app) {
  return pathToFileURL(path.join(root, "apps", `${app.kind}-stream-mini`, "ui", "index.html")).href;
}

async function frameState(page) {
  return page.locator(".node-surface").evaluate((node) => {
    const style = getComputedStyle(node);
    return {
      animation: style.animationName,
      border: style.borderTopColor,
      radius: style.borderTopLeftRadius,
      shadow: style.boxShadow,
    };
  });
}

async function assertNoOverflow(page) {
  const overflow = await page.evaluate(() => ({
    horizontal: document.documentElement.scrollWidth - window.innerWidth,
    vertical: document.documentElement.scrollHeight - window.innerHeight,
  }));
  assert.ok(overflow.horizontal <= 0, `horizontal overflow: ${overflow.horizontal}px`);
  assert.ok(overflow.vertical <= 0, `vertical overflow: ${overflow.vertical}px`);
}
