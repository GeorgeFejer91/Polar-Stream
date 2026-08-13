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
        "content-security-policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:",
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
  assert.equal(await desktop.locator("#runtime-path-label").textContent(), "NeuroKit simulated data");
  assert.match(await desktop.locator(".device-row.mock").textContent(), /SYNTHETIC/);
  assert.match(await desktop.locator(".device-row.mock").textContent(), /no Polar H10 required/);
  await connectMock(desktop);
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

  process.stdout.write(`Validated canonical Pages parity, NeuroKit replay, and responsive layouts in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
