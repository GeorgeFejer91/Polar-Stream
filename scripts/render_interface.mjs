import assert from "node:assert/strict";
import { mkdir, readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = normalize(fileURLToPath(new URL("../apps/polar-stream/ui/", import.meta.url)));
const output = normalize(fileURLToPath(new URL("../artifacts/interface-renderer/", import.meta.url)));
const targets = [
  ["breathing-phase-inhale", "INHALE", [22, 130, 89]],
  ["breathing-phase-exhale", "EXHALE", [209, 122, 40]],
  ["breathing-phase-pause", "PAUSE", [59, 120, 170]],
];

const mime = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".png", "image/png"],
]);

function startServer() {
  const server = createServer(async (request, response) => {
    try {
      const requestPath = new URL(request.url, "http://renderer.local").pathname;
      const relative = requestPath === "/" ? "index.html" : requestPath.slice(1);
      const path = normalize(join(root, relative));
      if (!path.startsWith(root)) throw new Error("Path outside renderer root");
      const body = await readFile(path);
      response.writeHead(200, { "content-type": mime.get(extname(path)) || "application/octet-stream" });
      response.end(body);
    } catch (_error) {
      response.writeHead(404);
      response.end("Not found");
    }
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

async function inspectCanvas(page) {
  return page.locator("#signal-canvas").evaluate((canvas) => {
    const context = canvas.getContext("2d");
    const { data, width, height } = context.getImageData(0, 0, canvas.width, canvas.height);
    let left = width;
    let right = -1;
    let top = height;
    let bottom = -1;
    let red = 0;
    let green = 0;
    let blue = 0;
    let opaque = 0;
    for (let index = 0; index < data.length; index += 4) {
      const alpha = data[index + 3];
      if (alpha === 0) continue;
      const pixel = index / 4;
      const x = pixel % width;
      const y = Math.floor(pixel / width);
      left = Math.min(left, x);
      right = Math.max(right, x);
      top = Math.min(top, y);
      bottom = Math.max(bottom, y);
      if (alpha > 240) {
        red += data[index];
        green += data[index + 1];
        blue += data[index + 2];
        opaque += 1;
      }
    }
    return {
      width: right >= left ? right - left + 1 : 0,
      height: bottom >= top ? bottom - top + 1 : 0,
      color: opaque ? [red / opaque, green / opaque, blue / opaque] : [0, 0, 0],
    };
  });
}

function colorDistance(actual, expected) {
  return Math.hypot(...actual.map((channel, index) => channel - expected[index]));
}

async function inspectStackedCanvas(page, colors = [[23, 107, 158], [42, 168, 184], [33, 138, 171]]) {
  return page.locator("#signal-canvas").evaluate((canvas, expectedColors) => {
    const context = canvas.getContext("2d");
    const { data, width } = context.getImageData(0, 0, canvas.width, canvas.height);
    return expectedColors.map((expected) => {
      let count = 0;
      let yTotal = 0;
      for (let index = 0; index < data.length; index += 4) {
        if (data[index + 3] < 100) continue;
        const distance = Math.hypot(
          data[index] - expected[0],
          data[index + 1] - expected[1],
          data[index + 2] - expected[2],
        );
        if (distance >= 24) continue;
        count += 1;
        yTotal += Math.floor(index / 4 / width);
      }
      return { count, averageY: count ? yTotal / count : 0 };
    });
  }, colors);
}

await mkdir(output, { recursive: true });
const server = await startServer();
const address = server.address();
const baseUrl = `http://127.0.0.1:${address.port}/index.html?renderer=1`;
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 1,
  locale: "en-US",
});

try {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.waitForFunction(() => Boolean(window.PolarInterfaceRenderer));
  await page.evaluate(() => window.PolarInterfaceRenderer.ready());
  const emptyState = await page.evaluate(() => ({
    profile: document.body.dataset.deviceProfile,
    outputEmptyVisible: !document.querySelector("#output-empty-state").hidden,
    outputWorkspaceHidden: document.querySelector("#output-workspace").hidden,
    visualEmptyVisible: !document.querySelector("#visual-empty-state").hidden,
    visualWorkspaceHidden: document.querySelector("#visual-workspace").hidden,
    outputState: document.querySelector("#output-state").textContent,
    visualChoices: [...document.querySelector("#visual-source").options].map((option) => option.value),
    outputCards: document.querySelector("#output-chips").children.length,
    connectedDeviceWidgets: document.querySelectorAll("#connected-device-list .connected-device-widget").length,
    connectedDeviceToggles: document.querySelectorAll("#connected-device-list .device-widget-toggle").length,
  }));
  assert.deepEqual(emptyState, {
    profile: "none",
    outputEmptyVisible: true,
    outputWorkspaceHidden: true,
    visualEmptyVisible: true,
    visualWorkspaceHidden: true,
    outputState: "Waiting",
    visualChoices: [],
    outputCards: 0,
    connectedDeviceWidgets: 0,
    connectedDeviceToggles: 0,
  }, "outputs or visualizations were instantiated before a device connected");
  const emptyScreenshot = join(output, "empty-device-protocols.png");
  await page.screenshot({ path: emptyScreenshot, fullPage: true });
  assert.ok((await stat(emptyScreenshot)).size > 20_000, "empty protocol screenshot was unexpectedly empty");
  const measurements = new Map();
  for (const [scenario, label, expectedColor] of targets) {
    const result = await page.evaluate((name) => window.PolarInterfaceRenderer.render(name), scenario);
    assert.equal(result.currentLabel, label, `${scenario} rendered the wrong class label`);
    assert.equal(result.selectedVisual, "breathing_phase");
    assert.match(result.streamName, /_breathingPhase$/);
    assert.match(await page.locator("#chart-shell").getAttribute("class"), /\bphase-visual\b/);
    const canvas = await inspectCanvas(page);
    assert.ok(canvas.width > 100 && canvas.height > 100, `${scenario} did not render a circle`);
    assert.ok(colorDistance(canvas.color, expectedColor) < 55, `${scenario} rendered the wrong phase color: ${canvas.color}`);
    measurements.set(scenario, canvas);
    const screenshot = join(output, `${scenario}.png`);
    await page.screenshot({ path: screenshot, fullPage: true });
    assert.ok((await stat(screenshot)).size > 20_000, `${scenario} screenshot was unexpectedly empty`);
  }

  assert.ok(
    measurements.get("breathing-phase-inhale").width > measurements.get("breathing-phase-exhale").width * 1.35,
    "inhale circle must render materially larger than exhale",
  );
  const paused = await page.evaluate(() => window.PolarInterfaceRenderer.render("breathing-phase-pause"));
  assert.ok(Math.abs(paused.phaseMotion.velocity) < 0.02, "pause should ease the circle velocity toward rest");
  assert.ok(paused.phaseMotion.level > 0.58, "pause should retain motion inertia instead of abruptly freezing");

  const settings = await page.evaluate(() => window.PolarInterfaceRenderer.render("breathing-phase-settings"));
  assert.equal(settings.dialogOpen, true);
  assert.equal(await page.locator("#module-dialog-title").textContent(), "Adjust Breath phase classifier");
  assert.ok(await page.locator("#module-settings input").count() >= 7, "classifier controls were not rendered");
  assert.equal(await page.getByLabel("X axis · recommended").isChecked(), true);
  assert.equal(await page.getByLabel("Y axis · rotational").isChecked(), false);
  assert.equal(await page.getByLabel("Z axis · recommended").isChecked(), true);
  assert.equal(await page.getByLabel("Volume algorithm").inputValue(), "timed-pca-v1");
  assert.equal(await page.getByLabel("Phase algorithm").inputValue(), "hysteresis-v1");
  assert.equal(await page.getByLabel("Timed volume filter tau").inputValue(), "0.18");
  assert.equal(await page.getByLabel("Phase enter threshold").inputValue(), "0.03");
  assert.equal(await page.getByLabel("Breathing display mode").inputValue(), "fresh-smooth");
  assert.equal(await page.getByLabel("Display delay").inputValue(), "0.18");
  await page.screenshot({ path: join(output, "breathing-phase-settings.png"), fullPage: true });
  await page.getByLabel("Display window").fill("12");
  await page.getByRole("button", { name: "Save module" }).click();
  const saved = await page.evaluate(() => window.PolarInterfaceRenderer.metricOptions("breathing_phase"));
  assert.equal(saved.displayWindowSeconds, 12, "Save module did not persist the rendered control value");
  assert.equal(await page.locator("#visual-window-label").textContent(), "Live phase");
  await page.screenshot({ path: join(output, "breathing-phase-settings-saved.png"), fullPage: true });

  const breathingTrail = await page.evaluate(() => window.PolarInterfaceRenderer.render("breathing-waveform-trail"));
  assert.equal(breathingTrail.selectedVisual, "breathing_volume");
  assert.match(breathingTrail.chartClass, /breathing-trail-visual/);
  assert.match(breathingTrail.canvasLabel, /moving dot.*leftward trail/i);
  assert.equal(breathingTrail.visualMode, "breathing-trail");
  assert.equal(breathingTrail.direction, "inhale");
  assert.ok(breathingTrail.trailPoints >= 90, `breathing trail is too short: ${breathingTrail.trailPoints}`);
  assert.ok(breathingTrail.latestY01 >= 0 && breathingTrail.latestY01 <= 1);
  assert.match(breathingTrail.currentLabel, /^0\.\d{3}$/);
  const breathingCanvas = await inspectCanvas(page);
  assert.ok(breathingCanvas.width > 500 && breathingCanvas.height > 150, "breathing dot and trail did not span the canvas");
  const breathingScreenshot = join(output, "breathing-waveform-trail.png");
  await page.screenshot({ path: breathingScreenshot, fullPage: true });
  assert.ok((await stat(breathingScreenshot)).size > 20_000, "breathing trail screenshot was unexpectedly empty");

  const accelerometer = await page.evaluate(() => window.PolarInterfaceRenderer.render("raw-accelerometer-stacked"));
  assert.equal(accelerometer.selectedVisual, "raw_acc");
  assert.deepEqual(accelerometer.legendLabels, ["X", "Y", "Z"]);
  assert.ok(accelerometer.visualOptions.includes("raw_acc"), "raw ACC is missing from the visualizer");
  assert.ok(!accelerometer.visualOptions.some((id) => /^acc_[xyz]$/.test(id)), "individual ACC axes remain selectable");
  assert.match(accelerometer.currentLabel, /^X [-\d]+  ·  Y [-\d]+  ·  Z [-\d]+$/);
  assert.match(accelerometer.chartClass, /stacked-axes/);
  assert.match(accelerometer.canvasLabel, /three stacked plots/);
  const stackedColors = await inspectStackedCanvas(page);
  assert.ok(stackedColors.every(({ count }) => count > 40), `one or more ACC traces were not drawn: ${JSON.stringify(stackedColors)}`);
  assert.ok(stackedColors[0].averageY < stackedColors[1].averageY && stackedColors[1].averageY < stackedColors[2].averageY,
    `ACC traces were not stacked X/Y/Z: ${JSON.stringify(stackedColors)}`);
  const accelerometerScreenshot = join(output, "raw-accelerometer-stacked.png");
  await page.screenshot({ path: accelerometerScreenshot, fullPage: true });
  assert.ok((await stat(accelerometerScreenshot)).size > 20_000, "stacked ACC screenshot was unexpectedly empty");

  const multipleSources = await page.evaluate(() => window.PolarInterfaceRenderer.render("multiple-colored-sources"));
  assert.deepEqual(multipleSources.sourceOptions, ["source-1", "source-2"]);
  assert.deepEqual(multipleSources.chipColors, ["#176B9E", "#B83E35"]);
  assert.deepEqual(multipleSources.connectedWidgets, [
    { sourceId: "source-1", profile: "polar", color: "#176B9E", hasKeepConnected: false, keepConnected: null },
    { sourceId: "source-2", profile: "vernier", color: "#B83E35", hasKeepConnected: true, keepConnected: true },
  ]);
  assert.equal(multipleSources.palettePickerCount, 2);
  assert.ok(multipleSources.availableDeviceCount >= 1, "available devices disappeared after sources connected");
  assert.equal(multipleSources.selectedSource, "source-2");
  assert.equal(multipleSources.chartColor, "#B83E35");
  assert.ok(multipleSources.outputColors.every((color) => color === "#B83E35"));
  assert.notEqual(multipleSources.forceValue, "—");
  assert.match(multipleSources.breathingValue, /^(?:0\.\d{3}|1\.000)$/);
  assert.equal(multipleSources.selectedVisual, "vernier_breathing");
  assert.deepEqual(multipleSources.visualOptions, ["raw_force", "vernier_breathing"]);
  assert.deepEqual(multipleSources.comparisonOptions, [""]);
  assert.equal(multipleSources.comparisonHidden, true, "comparison control appeared without an active compatible signal");
  assert.equal(multipleSources.deviceProfile, "vernier");
  assert.match(multipleSources.deviceProfileTitle, /respiration belt/i);
  assert.deepEqual(multipleSources.rawCardVisibility, { ecg: false, acc: false, force: true, breathing: true });
  assert.deepEqual(multipleSources.libraryIds, ["raw_force"]);
  assert.equal(multipleSources.formulaLabHidden, true);
  assert.equal(multipleSources.protocolCardCount, 2);
  assert.match(multipleSources.streamName, /_source-2_rawForce$/);
  assert.deepEqual(multipleSources.scanAction, {
    label: "Add another sensor",
    disabled: false,
    caption: "All active streams continue while discovery runs",
  });
  assert.equal(multipleSources.polarSwitch.deviceProfile, "polar");
  assert.ok(multipleSources.polarSwitch.visualOptions.includes("raw_ecg"));
  assert.ok(multipleSources.polarSwitch.visualOptions.includes("raw_acc"));
  assert.ok(!multipleSources.polarSwitch.visualOptions.some((id) => id.startsWith("compare_")));
  assert.ok(!multipleSources.polarSwitch.visualOptions.includes("raw_force"));
  assert.ok(!multipleSources.polarSwitch.visualOptions.includes("vernier_breathing"));
  assert.ok(multipleSources.polarSwitch.outputLabels.includes("Raw ECG"));
  assert.ok(multipleSources.polarSwitch.outputLabels.includes("Raw accelerometer"));
  assert.equal(multipleSources.polarSwitch.automaticRawCount, 2);
  assert.ok(!multipleSources.polarSwitch.outputLabels.includes("Raw Go Direct force"));
  assert.deepEqual(multipleSources.polarSwitch.rawCardVisibility, { ecg: true, acc: true, force: false, breathing: false });
  await page.getByLabel("Color pair for GDX-RB A").selectOption("meadow");
  const recolored = await page.evaluate(() => ({
    widget: document.querySelector('[data-source-id="source-2"]').style.getPropertyValue("--source-color"),
    chart: document.querySelector("#chart-shell").style.getPropertyValue("--source-color"),
    outputs: [...document.querySelector("#output-chips").children].map((card) => card.style.getPropertyValue("--source-color")),
    outputPanelMarked: document.querySelector("#output-workspace").classList.contains("source-panel-marked"),
    visualPanelMarked: document.querySelector("#visual-workspace").classList.contains("source-panel-marked"),
  }));
  assert.equal(recolored.widget, "#4E7B27");
  assert.equal(recolored.chart, "#4E7B27");
  assert.ok(recolored.outputs.every((color) => color === "#4E7B27"));
  assert.equal(recolored.outputPanelMarked, true);
  assert.equal(recolored.visualPanelMarked, true);
  await page.screenshot({ path: join(output, "multiple-colored-sources.png"), fullPage: true });

  const comparison = await page.evaluate(() => window.PolarInterfaceRenderer.render("multi-source-comparison"));
  assert.equal(comparison.selectedVisual, "breathing_volume");
  assert.ok(comparison.visualOptions.includes("breathing_volume"));
  assert.deepEqual(comparison.comparisonOptions, ["", "source-2"]);
  assert.deepEqual(comparison.incompatibleComparisonOptions, { raw_ecg: [""], raw_acc: [""] });
  assert.equal(comparison.visualMode, "time-aligned-comparison");
  assert.equal(comparison.composite, "breathing_waveform_01");
  assert.match(comparison.currentLabel, /^Source 1 0\.\d{3} · Source 2 0\.\d{3}$/);
  assert.match(comparison.canvasLabel, /time-aligned comparison/i);
  assert.match(comparison.chartClass, /stacked-axes/);
  assert.deepEqual(comparison.legendLabels, ["Source 1 · Polar H10 A", "Source 2 · GDX-RB A"]);
  const comparisonCanvas = await inspectCanvas(page);
  assert.ok(comparisonCanvas.width > 500 && comparisonCanvas.height > 150, `time-aligned comparison did not span the canvas: ${JSON.stringify(comparisonCanvas)}`);
  const comparisonScreenshot = join(output, "multi-source-comparison.png");
  await page.screenshot({ path: comparisonScreenshot, fullPage: true });
  assert.ok((await stat(comparisonScreenshot)).size > 20_000, "comparison screenshot was unexpectedly empty");

  const accLibrary = await page.evaluate(() => window.PolarInterfaceRenderer.render("acc-primary-library"));
  assert.deepEqual(accLibrary.primaryIds, ["raw_acc", "acc_magnitude", "breathing_volume"]);
  for (const retainedId of [
    "acc_breathing_magnitude", "breathing_phase", "breathing_signal_ready",
    "breathing_signal_confidence", "breathing_calibration", "breathing_axis_range",
    "breathing_rate", "breathing_dynamics_confidence",
  ]) {
    assert.ok(accLibrary.extraIds.includes(retainedId), `${retainedId} is missing from Extra options`);
  }

  const library = await page.evaluate(() => window.PolarInterfaceRenderer.render("metric-library-previews"));
  assert.equal(library.dialogOpen, true, "metric library did not open in the renderer");
  assert.equal(library.previewCount, library.catalogCount - 1, "fixture-backed preview count differs from catalog count");
  assert.deepEqual(library.missingPreviewIds, ["raw_force"], "only the live-hardware Go Direct signal may lack a recorded H10 preview");
  assert.equal(library.source.library, "Recorded Polar H10");
  assert.equal(library.source.version, "60-second anonymized fixture");
  assert.equal(library.source.model, "real-polar-h10-recording");
  assert.match(library.source.fixtureSha256, /^[a-f0-9]{64}$/);
  assert.equal(await page.locator(".metric-option .metric-preview-svg").count(), 0, "metric rows must not animate before selection");
  assert.ok(!library.visibleIds.includes("raw_acc"), "ACC outputs leaked into ECG mode");
  assert.ok(!library.visibleIds.includes("breathing_phase"), "ACC breathing leaked into ECG mode");
  for (const metric of library.detailCoverage) {
    assert.ok(metric.sentenceCount >= 2 && metric.sentenceCount <= 3, `${metric.id} summary is not two or three sentences`);
    assert.ok(metric.sourceCount >= 2 && metric.sourceCount <= 3, `${metric.id} does not have two or three sources`);
    assert.equal(new Set(metric.sourceUrls).size, metric.sourceCount, `${metric.id} repeats a source`);
    assert.ok(metric.sourceUrls.every((url) => /^https:\/\//.test(url)), `${metric.id} has a non-HTTPS source`);
  }

  for (const metricId of ["raw_ecg", "rmssd", "excitement_score"]) {
    await page.locator(`.metric-option[data-metric-id="${metricId}"]`).click();
    const detail = page.locator(`.metric-preview-large[data-metric-id="${metricId}"]`);
    assert.equal(await detail.count(), 1, `${metricId} did not render a selected-metric preview`);
    assert.ok(await detail.locator(".metric-preview-line").count() >= 2, `${metricId} preview path is missing`);
    assert.equal(await detail.locator("animateTransform").count(), 1, `${metricId} preview is not looped`);
    assert.equal(await page.locator("#metric-detail article > section").count(), 3, `${metricId} detail contains extra sections`);
    assert.equal(await page.locator(".metric-scientific-summary").count(), 1);
    const sourceLinks = page.locator(".metric-source-list a");
    const sourceLinkCount = await sourceLinks.count();
    assert.ok(sourceLinkCount >= 2 && sourceLinkCount <= 3, `${metricId} source list has the wrong size`);
    assert.equal(await page.locator(".metric-preview-settings, .metric-formula-context, .metric-stream-preview, .breathing-selection-settings").count(), 0);
  }
  await page.getByRole("button", { name: /ACC metrics/ }).click();
  assert.equal(await page.locator("#output-dialog").getAttribute("data-family"), "acc");
  const primaryAccIds = await page.locator(".metric-option").evaluateAll((options) => options.map((option) => option.dataset.metricId));
  assert.deepEqual(primaryAccIds, ["raw_acc", "acc_magnitude", "breathing_volume"]);
  await page.getByRole("button", { name: "Extra options" }).click();
  const accIds = await page.locator(".metric-option").evaluateAll((options) => options.map((option) => option.dataset.metricId));
  assert.ok(accIds.length > 20, `complete specialist ACC and breathing catalog was not exposed: ${JSON.stringify(accIds)}`);
  assert.ok(!accIds.includes("raw_acc") && accIds.includes("breathing_phase") && accIds.includes("breath_interval_sampen"));
  assert.match(await page.locator("#metric-library-summary").textContent(), new RegExp(`^${accIds.length} of ${accIds.length} ACC metrics$`));

  for (const metricId of ["acc_breathing_magnitude", "breathing_phase", "breath_interval_sampen"]) {
    await page.locator(`.metric-option[data-metric-id="${metricId}"]`).click();
    assert.equal(await page.locator(`.metric-preview-large[data-metric-id="${metricId}"] animateTransform`).count(), 1);
    assert.equal(await page.locator(".metric-scientific-summary").count(), 1);
    assert.ok(await page.locator(".metric-source-list a").count() >= 2);
    assert.equal(await page.locator("#metric-detail article > section").count(), 3, `${metricId} detail contains extra sections`);
  }
  const previewScreenshot = join(output, "metric-library-previews.png");
  await page.screenshot({ path: previewScreenshot, fullPage: true });
  assert.ok((await stat(previewScreenshot)).size > 20_000, "metric library screenshot was unexpectedly empty");

  await page.locator("#output-dialog").evaluate((dialog) => dialog.close());
  await page.evaluate(() => window.PolarInterfaceRenderer.render("multiple-colored-sources"));
  for (const [theme, expectedColor] of [["light", "#176B9E"], ["dark", "#7CCBFF"]]) {
    const wantsDark = theme === "dark";
    const isDark = await page.locator("html").getAttribute("data-theme") === "dark";
    if (isDark !== wantsDark) await page.locator("#theme-toggle").click();
    assert.equal(await page.locator("#theme-toggle").getAttribute("aria-pressed"), String(wantsDark));
    assert.equal(await page.locator("meta[name='theme-color']").getAttribute("content"), wantsDark ? "#202428" : "#17221d");
    await page.evaluate(() => window.PolarInterfaceRenderer.render("multiple-colored-sources"));
    await page.evaluate(() => document.querySelector('[data-source-id="source-1"] button').click());
    assert.equal(await page.locator("#chart-shell").evaluate((node) => node.style.getPropertyValue("--source-color")), expectedColor);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width === 1440 ? 900 : 780 });
      const screenshot = join(output, `${theme}-${width}.png`);
      await page.screenshot({ path: screenshot, fullPage: true });
      assert.ok((await stat(screenshot)).size > 15_000, `${theme} ${width}px screenshot was unexpectedly empty`);
    }
  }

  const firstPaintTheme = await browser.newPage({ viewport: { width: 390, height: 780 }, colorScheme: "dark" });
  await firstPaintTheme.goto(baseUrl, { waitUntil: "domcontentloaded" });
  assert.equal(await firstPaintTheme.locator("html").getAttribute("data-theme"), "dark", "OS dark preference was not applied before app initialization");
  await firstPaintTheme.locator("#theme-toggle").click();
  assert.equal(await firstPaintTheme.evaluate(() => localStorage.getItem("polar-stream.theme.v1")), "light");
  await firstPaintTheme.emulateMedia({ colorScheme: "dark" });
  await firstPaintTheme.reload({ waitUntil: "domcontentloaded" });
  assert.equal(await firstPaintTheme.locator("html").getAttribute("data-theme"), "light", "explicit theme preference did not override the OS setting");
  await firstPaintTheme.close();

  process.stdout.write(`Validated primary/extra ACC outputs, safe breathing comparison, source palettes, light/dark desktop and mobile states, ${targets.length} classifier renders, and ${library.previewCount} metric previews in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
