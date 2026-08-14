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

async function inspectStackedCanvas(page) {
  const colors = [[59, 120, 170], [22, 130, 89], [166, 109, 25]];
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
        if (distance >= 42) continue;
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
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });

try {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.waitForFunction(() => Boolean(window.PolarInterfaceRenderer));
  const measurements = new Map();
  for (const [scenario, label, expectedColor] of targets) {
    const result = await page.evaluate((name) => window.PolarInterfaceRenderer.render(name), scenario);
    assert.equal(result.currentLabel, label, `${scenario} rendered the wrong class label`);
    assert.equal(result.selectedVisual, "breathing_phase");
    assert.match(result.streamName, /_breathingPhase$/);
    assert.equal(await page.locator("#chart-shell").getAttribute("class"), "chart-shell phase-visual");
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
  assert.equal(await page.getByLabel("Sensitivity").inputValue(), "0.6");
  await page.screenshot({ path: join(output, "breathing-phase-settings.png"), fullPage: true });
  await page.getByLabel("Display window").fill("12");
  await page.getByRole("button", { name: "Save module" }).click();
  const saved = await page.evaluate(() => window.PolarInterfaceRenderer.metricOptions("breathing_phase"));
  assert.equal(saved.displayWindowSeconds, 12, "Save module did not persist the rendered control value");
  assert.equal(await page.locator("#visual-window-label").textContent(), "Live phase");
  await page.screenshot({ path: join(output, "breathing-phase-settings-saved.png"), fullPage: true });

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

  const library = await page.evaluate(() => window.PolarInterfaceRenderer.render("metric-library-previews"));
  assert.equal(library.dialogOpen, true, "metric library did not open in the renderer");
  assert.equal(library.previewCount, library.catalogCount, "generated preview count differs from catalog count");
  assert.deepEqual(library.missingPreviewIds, [], "one or more catalog metrics lack a generated preview");
  assert.equal(library.source.library, "NeuroKit2");
  assert.equal(library.source.version, "0.2.13");
  assert.equal(library.source.model, "ECGSYN");
  assert.equal(await page.locator(".metric-option .metric-preview-svg").count(), library.visibleCount);
  assert.equal(await page.locator(".metric-preview-missing").count(), 0);
  assert.ok(!library.visibleIds.includes("raw_acc"), "ACC outputs leaked into ECG mode");
  assert.ok(!library.visibleIds.includes("breathing_phase"), "ACC breathing leaked into ECG mode");
  const coverage = await page.locator(".metric-preview-compact").evaluateAll((figures) => figures.map((figure) => ({
    id: figure.dataset.metricId,
    paths: figure.querySelectorAll("path.metric-preview-line").length,
    pathLength: [...figure.querySelectorAll("path.metric-preview-line")]
      .reduce((sum, path) => sum + (path.getAttribute("d")?.length || 0), 0),
    minimum: Number(figure.dataset.minimum),
    maximum: Number(figure.dataset.maximum),
  })));
  assert.equal(new Set(coverage.map((preview) => preview.id)).size, library.visibleCount);
  for (const preview of coverage) {
    assert.ok(preview.paths >= 1, `${preview.id} has no generated SVG path`);
    assert.ok(preview.pathLength > 120, `${preview.id} SVG path was unexpectedly small`);
    assert.ok(Number.isFinite(preview.minimum) && Number.isFinite(preview.maximum), `${preview.id} has no finite range`);
    assert.ok(preview.maximum >= preview.minimum, `${preview.id} has an inverted range`);
  }

  for (const metricId of ["raw_ecg", "rmssd", "excitement_score"]) {
    await page.locator(`.metric-option:has(.metric-preview[data-metric-id="${metricId}"])`).click();
    const detail = page.locator(`.metric-preview-large[data-metric-id="${metricId}"]`);
    assert.equal(await detail.count(), 1, `${metricId} did not render a selected-metric preview`);
    assert.ok(await detail.locator(".metric-preview-line").count() >= 2, `${metricId} preview path is missing`);
    assert.equal(await detail.locator("animateTransform").count(), 1, `${metricId} preview is not looped`);
  }
  await page.getByRole("button", { name: /ACC metrics/ }).click();
  assert.equal(await page.locator("#output-dialog").getAttribute("data-family"), "acc");
  const accIds = await page.locator(".metric-option").evaluateAll((options) => options.map((option) => option.dataset.metricId));
  assert.deepEqual(accIds, ["raw_acc", "acc_magnitude", "acc_breathing_magnitude", "breathing_phase"]);
  assert.match(await page.locator("#metric-library-summary").textContent(), /^4 of 4 ACC metrics$/);

  await page.locator('.metric-option[data-metric-id="acc_breathing_magnitude"]').click();
  assert.equal(await page.locator(".experimental-badge").textContent(), "Not validated");
  const selectionSettings = page.locator(".breathing-selection-settings");
  assert.equal(await selectionSettings.getByLabel("Normalize output to 0–1").isChecked(), true);
  assert.equal(await selectionSettings.getByLabel("X · recommended").isChecked(), true);
  assert.equal(await selectionSettings.getByLabel("Y · rotational").isChecked(), false);
  assert.equal(await selectionSettings.getByLabel("Z · recommended").isChecked(), true);

  await page.locator('.metric-option[data-metric-id="breathing_phase"]').click();
  assert.equal(await selectionSettings.getByLabel("Sensitivity").inputValue(), "0.6");
  assert.equal(await selectionSettings.getByLabel("Invert inhale / exhale").isChecked(), false);
  assert.match(await page.locator("#metric-detail").textContent(), /inhale \(\+1\), pause\/not ready \(0\), and exhale \(−1\)/);
  assert.match(await page.locator(".metric-preview-provenance").textContent(), /NeuroKit2 0\.2\.13 · ECGSYN/);
  assert.match(await page.locator(".metric-preview-note").textContent(), /not expected personal values or validation accuracy/);
  const previewScreenshot = join(output, "metric-library-previews.png");
  await page.screenshot({ path: previewScreenshot, fullPage: true });
  assert.ok((await stat(previewScreenshot)).size > 20_000, "metric library screenshot was unexpectedly empty");

  process.stdout.write(`Validated stacked raw ACC, ${targets.length} classifier renders, saved controls, and ${library.catalogCount} NeuroKit previews in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
