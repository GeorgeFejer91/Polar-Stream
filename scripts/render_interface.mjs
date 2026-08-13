import assert from "node:assert/strict";
import { mkdir, readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import process from "node:process";
import { chromium } from "playwright";

const root = normalize(new URL("../apps/polar-stream/ui/", import.meta.url).pathname);
const output = normalize(new URL("../artifacts/interface-renderer/", import.meta.url).pathname);
const targets = [
  ["breathing-phase-inhale", "INHALE", [22, 130, 89]],
  ["breathing-phase-exhale", "EXHALE", [209, 122, 40]],
  ["breathing-phase-pause", "PAUSE", [59, 120, 170]],
  ["breathing-phase-bad-signal", "BAD SIGNAL", [185, 75, 64]],
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

  const settings = await page.evaluate(() => window.PolarInterfaceRenderer.render("breathing-phase-settings"));
  assert.equal(settings.dialogOpen, true);
  assert.equal(await page.locator("#module-dialog-title").textContent(), "Adjust Breath phase classifier");
  assert.ok(await page.locator("#module-settings input").count() >= 10, "classifier controls were not rendered");
  await page.screenshot({ path: join(output, "breathing-phase-settings.png"), fullPage: true });
  await page.getByLabel("Display window").fill("12");
  await page.getByRole("button", { name: "Save module" }).click();
  const saved = await page.evaluate(() => window.PolarInterfaceRenderer.metricOptions("breathing_phase"));
  assert.equal(saved.displayWindowSeconds, 12, "Save module did not persist the rendered control value");
  assert.equal(await page.locator("#visual-window-label").textContent(), "Live phase");
  await page.screenshot({ path: join(output, "breathing-phase-settings-saved.png"), fullPage: true });

  const library = await page.evaluate(() => window.PolarInterfaceRenderer.render("metric-library-previews"));
  assert.equal(library.dialogOpen, true, "metric library did not open in the renderer");
  assert.equal(library.previewCount, library.catalogCount, "generated preview count differs from catalog count");
  assert.deepEqual(library.missingPreviewIds, [], "one or more catalog metrics lack a generated preview");
  assert.equal(library.source.library, "NeuroKit2");
  assert.equal(library.source.version, "0.2.13");
  assert.equal(library.source.model, "ECGSYN");
  assert.equal(await page.locator(".metric-option .metric-preview-svg").count(), library.catalogCount);
  assert.equal(await page.locator(".metric-preview-missing").count(), 0);
  const coverage = await page.locator(".metric-preview-compact").evaluateAll((figures) => figures.map((figure) => ({
    id: figure.dataset.metricId,
    paths: figure.querySelectorAll("path.metric-preview-line").length,
    pathLength: [...figure.querySelectorAll("path.metric-preview-line")]
      .reduce((sum, path) => sum + (path.getAttribute("d")?.length || 0), 0),
    minimum: Number(figure.dataset.minimum),
    maximum: Number(figure.dataset.maximum),
  })));
  assert.equal(new Set(coverage.map((preview) => preview.id)).size, library.catalogCount);
  for (const preview of coverage) {
    assert.ok(preview.paths >= 1, `${preview.id} has no generated SVG path`);
    assert.ok(preview.pathLength > 120, `${preview.id} SVG path was unexpectedly small`);
    assert.ok(Number.isFinite(preview.minimum) && Number.isFinite(preview.maximum), `${preview.id} has no finite range`);
    assert.ok(preview.maximum >= preview.minimum, `${preview.id} has an inverted range`);
  }

  for (const metricId of ["raw_ecg", "raw_acc", "rmssd", "breathing_phase", "excitement_score"]) {
    await page.locator(`.metric-option:has(.metric-preview[data-metric-id="${metricId}"])`).click();
    const detail = page.locator(`.metric-preview-large[data-metric-id="${metricId}"]`);
    assert.equal(await detail.count(), 1, `${metricId} did not render a selected-metric preview`);
    assert.ok(await detail.locator(".metric-preview-line").count() >= 2, `${metricId} preview path is missing`);
    assert.equal(await detail.locator("animateTransform").count(), 1, `${metricId} preview is not looped`);
  }
  assert.match(await page.locator(".metric-preview-provenance").textContent(), /NeuroKit2 0\.2\.13 · ECGSYN/);
  assert.match(await page.locator(".metric-preview-note").textContent(), /not expected personal values or validation accuracy/);
  const previewScreenshot = join(output, "metric-library-previews.png");
  await page.screenshot({ path: previewScreenshot, fullPage: true });
  assert.ok((await stat(previewScreenshot)).size > 20_000, "metric library screenshot was unexpectedly empty");

  process.stdout.write(`Validated ${targets.length} classifier renders, saved controls, and ${library.catalogCount} NeuroKit previews in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
