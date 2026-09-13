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

async function textContrast(page, selector) {
  return page.locator(selector).first().evaluate((element) => {
    const parseColor = (value) => {
      const channels = value.match(/[\d.]+/g)?.map(Number) || [];
      const srgb = value.startsWith("color(srgb");
      return {
        rgb: channels.slice(0, 3).map((channel) => srgb ? channel * 255 : channel),
        alpha: channels[3] ?? 1,
      };
    };
    const luminance = (rgb) => {
      const linear = rgb.map((channel) => {
        const value = channel / 255;
        return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
    };
    const foreground = parseColor(getComputedStyle(element).color).rgb;
    let backgroundNode = element;
    let background = parseColor(getComputedStyle(backgroundNode).backgroundColor);
    while (background.alpha === 0 && backgroundNode.parentElement) {
      backgroundNode = backgroundNode.parentElement;
      background = parseColor(getComputedStyle(backgroundNode).backgroundColor);
    }
    const foregroundLuminance = luminance(foreground);
    const backgroundLuminance = luminance(background.rgb);
    return {
      ratio: (Math.max(foregroundLuminance, backgroundLuminance) + 0.05)
        / (Math.min(foregroundLuminance, backgroundLuminance) + 0.05),
      foreground,
      background: background.rgb,
    };
  });
}

async function assertTextContrast(page, selector, minimum = 4.5) {
  const contrast = await textContrast(page, selector);
  assert.ok(
    contrast.ratio >= minimum,
    `${selector} contrast ${contrast.ratio.toFixed(2)}:1 is below ${minimum}:1 (${contrast.foreground} on ${contrast.background})`,
  );
}

async function inspectCanvasTraceColors(page, colors) {
  return page.locator("#signal-canvas").evaluate((canvas, expectedColors) => {
    const context = canvas.getContext("2d");
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    return expectedColors.map((expected) => {
      let count = 0;
      for (let index = 0; index < data.length; index += 4) {
        if (data[index + 3] < 90) continue;
        const distance = Math.hypot(
          data[index] - expected[0],
          data[index + 1] - expected[1],
          data[index + 2] - expected[2],
        );
        if (distance < 22) count += 1;
      }
      return count;
    });
  }, colors);
}

async function settleWorkspaceLayout(page) {
  await page.evaluate(() => new Promise((resolve) => {
    window.requestAnimationFrame(() => window.requestAnimationFrame(resolve));
  }));
}

async function inspectWorkspaceLayout(page) {
  return page.evaluate(() => {
    const workspace = document.querySelector("#workspace");
    const panels = ["#input-section", "#output-section", "#visual-section"]
      .map((selector) => document.querySelector(selector).getBoundingClientRect());
    const dividers = ["#input-output-divider", "#output-visual-divider"].map((selector) => {
      const element = document.querySelector(selector);
      const bounds = element.getBoundingClientRect();
      const hitArea = getComputedStyle(element, "::after");
      const hitLeft = Number.parseFloat(hitArea.left) || 0;
      const hitRight = Number.parseFloat(hitArea.right) || 0;
      return {
        width: bounds.width,
        hitWidth: bounds.width - hitLeft - hitRight,
        display: getComputedStyle(element).display,
        tabIndex: element.tabIndex,
        role: element.getAttribute("role"),
        orientation: element.getAttribute("aria-orientation"),
        controls: element.getAttribute("aria-controls"),
        ariaHidden: element.getAttribute("aria-hidden"),
        ariaDisabled: element.getAttribute("aria-disabled"),
        valueMin: Number(element.getAttribute("aria-valuemin")),
        valueMax: Number(element.getAttribute("aria-valuemax")),
        valueNow: Number(element.getAttribute("aria-valuenow")),
        valueText: element.getAttribute("aria-valuetext"),
      };
    });
    const panelWidth = panels.reduce((sum, panel) => sum + panel.width, 0);
    let stored = null;
    try {
      stored = JSON.parse(localStorage.getItem("polar-stream.workspace-layout.v1") || "null");
    } catch (_error) {
      stored = null;
    }
    const chart = document.querySelector("#chart-shell").getBoundingClientRect();
    const canvas = document.querySelector("#signal-canvas");
    return {
      workspaceWidth: workspace.getBoundingClientRect().width,
      panelWidths: panels.map((panel) => panel.width),
      panelScrollWidths: ["#input-section", "#output-section", "#visual-section"]
        .map((selector) => document.querySelector(selector).scrollWidth),
      panelTops: panels.map((panel) => panel.top),
      proportions: panels.map((panel) => panel.width / panelWidth),
      dividerWidth: dividers.reduce((sum, divider) => sum + divider.width, 0),
      dividers,
      stored,
      chartWidth: chart.width,
      canvasCssWidth: canvas.getBoundingClientRect().width,
      canvasPixelWidth: canvas.width,
      pixelRatio: Math.min(window.devicePixelRatio || 1, 2),
    };
  });
}

async function dragWorkspaceDivider(page, selector, deltaX) {
  const bounds = await page.locator(selector).boundingBox();
  assert.ok(bounds, `${selector} has no draggable bounds`);
  const startX = bounds.x + bounds.width / 2;
  const y = bounds.y + Math.min(120, bounds.height / 2);
  await page.mouse.move(startX, y);
  await page.mouse.down();
  await page.mouse.move(startX + deltaX, y, { steps: 6 });
  await page.mouse.up();
  await settleWorkspaceLayout(page);
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
  const designBaseline = await page.evaluate(() => ({
    decorativeDashboardElements: document.querySelectorAll(".eyebrow, .section-number, .panel-empty-mark, .empty-orbit").length,
    bodyFontSize: Number.parseFloat(getComputedStyle(document.body).fontSize),
    mainCount: document.querySelectorAll("main").length,
  }));
  assert.equal(designBaseline.decorativeDashboardElements, 0, "decorative dashboard UI returned");
  assert.ok(designBaseline.bodyFontSize >= 14, "base UI copy is smaller than 14px");
  assert.equal(designBaseline.mainCount, 1, "the page must have one main landmark");
  const connectionContract = await page.evaluate(async () => ({
    policy: window.PolarInterfaceRenderer.connectionContractPolicy(),
    queue: await window.PolarInterfaceRenderer.probeConnectionQueue(),
  }));
  assert.deepEqual(connectionContract.policy, {
    streamingConfirmationRequired: true,
    readyTimeoutMilliseconds: 15_000,
    retryDelaysMilliseconds: [1_500, 3_000, 6_000, 12_000, 24_000],
  });
  assert.deepEqual(connectionContract.queue.order, [
    "start:polar-a", "end:polar-a",
    "start:vernier", "end:vernier",
    "start:polar-b", "end:polar-b",
  ], "selected-device connection attempts were not serialized");
  assert.equal(connectionContract.queue.maximumActive, 1, "more than one sensor setup ran concurrently");
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
  await page.locator("#node-view-toggle").click();
  const nodeToggle = await page.evaluate(() => ({
    mode: document.body.dataset.viewMode,
    panelsHidden: document.querySelector("#workspace").hidden,
    nodesHidden: document.querySelector("#node-workspace").hidden,
    nodePressed: document.querySelector("#node-view-toggle").getAttribute("aria-pressed"),
    panelPressed: document.querySelector("#panel-view-toggle").getAttribute("aria-pressed"),
    editorVisible: !document.querySelector("#node-editor").hidden,
    nodeIds: [...document.querySelectorAll(".patch-node")].map((node) => node.dataset.nodeId),
    nodeTypes: [...document.querySelectorAll(".patch-node")].map((node) => window.getComputedStyle(node).cursor ? node.querySelector("strong")?.textContent : ""),
    linkCount: document.querySelectorAll("#node-link-layer path").length,
    nodeSummary: document.querySelector("#node-view-summary").textContent,
    emptyVisible: !document.querySelector("#node-empty-state").hidden,
    actionLabels: [...document.querySelectorAll(".node-view-actions button span")].map((node) => node.textContent),
    actionIconCount: document.querySelectorAll(".node-view-actions button .node-action-icon").length,
    actionIconsHidden: [...document.querySelectorAll(".node-view-actions button .node-action-icon")]
      .every((node) => node.getAttribute("aria-hidden") === "true"),
  }));
  assert.deepEqual(nodeToggle, {
    mode: "nodes",
    panelsHidden: true,
    nodesHidden: false,
    nodePressed: "true",
    panelPressed: "false",
    editorVisible: true,
    nodeIds: [],
    nodeTypes: [],
    linkCount: 0,
    nodeSummary: "Patch field ready · add an input/source to begin",
    emptyVisible: true,
    actionLabels: [
      "Add input/source",
      "Add transformer",
      "Add output",
      "Visualizer nodes",
      "Reset view",
      "Open panels",
    ],
    actionIconCount: 6,
    actionIconsHidden: true,
  }, "node view toggle did not expose the interactive signal-flow node editor");
  const nodeActionColorCount = await page.locator(".node-view-actions .ps-coral, .node-view-actions .ps-orange, .node-view-actions .ps-yellow, .node-view-actions .ps-mint, .node-view-actions .ps-cyan, .node-view-actions .ps-blue").count();
  assert.ok(nodeActionColorCount >= 18, "node toolbar widgets did not retain the Polar Stream logo color set");
  await page.locator("#node-add-source-button").click();
  await page.locator("#node-menu-search").fill("polar");
  const nodeMenu = await page.evaluate(() => ({
    hidden: document.querySelector("#node-menu").hidden,
    focused: document.activeElement?.id,
    entries: [...document.querySelectorAll("#node-menu-list button strong")].map((node) => node.textContent),
    widgetCount: document.querySelectorAll("#node-menu-list button .node-menu-widget").length,
    codeBadgeCount: document.querySelectorAll("#node-menu-list button code").length,
    coloredWidgetMarks: document.querySelectorAll("#node-menu-list button .ps-coral, #node-menu-list button .ps-orange, #node-menu-list button .ps-yellow, #node-menu-list button .ps-mint, #node-menu-list button .ps-cyan, #node-menu-list button .ps-blue").length,
  }));
  assert.deepEqual(nodeMenu, {
    hidden: false,
    focused: "node-menu-search",
    entries: ["Polar H10 source", "Mock Polar H10"],
    widgetCount: 2,
    codeBadgeCount: 0,
    coloredWidgetMarks: 7,
  }, "node menu did not expose searchable patch nodes");
  await page.locator("#node-menu-list button", { hasText: "Mock Polar H10" }).click();
  const sourceNode = await page.evaluate(() => ({
    nodeTypes: [...document.querySelectorAll(".patch-node strong")].map((node) => node.textContent),
    outputPortLabels: [...document.querySelectorAll(".patch-node .node-port-row.output .node-port-label")].map((node) => node.textContent),
    emptyVisible: !document.querySelector("#node-empty-state").hidden,
  }));
  assert.deepEqual(sourceNode, {
    nodeTypes: ["Mock Polar H10"],
    outputPortLabels: ["ECG", "ACC", "HR"],
    emptyVisible: false,
  }, "source node did not expose default checked raw/source output ports");
  await page.locator(".patch-node button", { hasText: "Inspect" }).click();
  const sourceDialog = await page.evaluate(() => ({
    open: document.querySelector("#node-source-dialog").open,
    title: document.querySelector("#node-source-dialog-title").textContent,
    checked: [...document.querySelectorAll("#node-source-signal-list input")].map((input) => input.checked),
    streamStatus: document.querySelector("#node-source-stream-status").textContent,
  }));
  assert.deepEqual(sourceDialog, {
    open: true,
    title: "Mock Polar H10",
    checked: [true, true, true],
    streamStatus: "3/3 checked",
  }, "source node dialog did not default every source signal to included");
  await page.locator("#node-source-close").click();
  await page.locator(".patch-node", { hasText: "Mock Polar H10" }).locator("p").click();
  const sourceClickDialog = await page.evaluate(() => ({
    open: document.querySelector("#node-source-dialog").open,
    title: document.querySelector("#node-source-dialog-title").textContent,
  }));
  assert.deepEqual(sourceClickDialog, {
    open: true,
    title: "Mock Polar H10",
  }, "clicking a source node did not reopen its source popup");
  await page.locator("#node-source-close").click();
  await page.locator("#node-add-transformer-button").click();
  await page.locator("#node-menu-search").fill("acc");
  const transformerMenu = await page.evaluate(() => ({
    entries: [...document.querySelectorAll("#node-menu-list button strong")].map((node) => node.textContent),
    widgetCount: document.querySelectorAll("#node-menu-list button .node-menu-widget").length,
    lungPathCount: document.querySelectorAll("#node-menu-list [data-node-icon=\"polar-acc-transformer\"] .node-widget-lung").length,
    codeBadgeCount: document.querySelectorAll("#node-menu-list button code").length,
  }));
  assert.deepEqual(transformerMenu, {
    entries: ["Polar ACC transformer"],
    widgetCount: 1,
    lungPathCount: 2,
    codeBadgeCount: 0,
  }, "transformer menu did not expose the Polar ACC transformer node");
  await page.locator("#node-menu-list button", { hasText: "Polar ACC transformer" }).click();
  const transformerNodeWidget = await page.evaluate(() => ({
    cardWidgetCount: document.querySelectorAll(".patch-node [data-node-icon=\"polar-acc-transformer\"].node-card-widget").length,
    lungPathCount: document.querySelectorAll(".patch-node [data-node-icon=\"polar-acc-transformer\"].node-card-widget .node-widget-lung").length,
    accTextBadgeCount: [...document.querySelectorAll(".patch-node mark")].filter((node) => node.textContent === "ACC").length,
  }));
  assert.deepEqual(transformerNodeWidget, {
    cardWidgetCount: 1,
    lungPathCount: 2,
    accTextBadgeCount: 0,
  }, "Polar ACC transformer node did not render its breath-shaped SVG widget");
  await page.locator(".patch-node", { hasText: "Polar ACC transformer" }).locator("p").click();
  const transformerDialog = await page.evaluate(() => ({
    open: document.querySelector("#node-inspector-dialog").open,
    title: document.querySelector("#node-inspector-title").textContent,
    subtitle: document.querySelector("#node-inspector-subtitle").textContent,
    state: document.querySelector("#node-inspector-state").textContent,
    action: document.querySelector("#node-inspector-action-button").textContent,
    portRows: document.querySelectorAll("#node-inspector-port-list .node-inspector-port-row").length,
  }));
  assert.deepEqual(transformerDialog, {
    open: true,
    title: "Polar ACC transformer",
    subtitle: "Transformer",
    state: "Awaiting input",
    action: "Open Output",
    portRows: 2,
  }, "clicking a transformer node did not open the node inspector popup");
  await page.locator("#node-inspector-close").click();
  await page.locator("#node-add-output-button").click();
  const outputMenu = await page.evaluate(() => ({
    entries: [...document.querySelectorAll("#node-menu-list button strong")].map((node) => node.textContent),
    widgetCount: document.querySelectorAll("#node-menu-list button .node-menu-widget").length,
    codeBadgeCount: document.querySelectorAll("#node-menu-list button code").length,
  }));
  assert.deepEqual(outputMenu, {
    entries: ["LSL recorder", "OSC sender", "Local CSV recorder", "PCM audio modem", "LabRecorder"],
    widgetCount: 5,
    codeBadgeCount: 0,
  }, "output node menu did not render every output option with a colored SVG widget");
  await page.locator("#node-menu-list button", { hasText: "LSL recorder" }).click();
  await page.locator(".patch-node", { hasText: "LSL recorder" }).locator("p").click();
  const outputDialog = await page.evaluate(() => ({
    open: document.querySelector("#node-inspector-dialog").open,
    title: document.querySelector("#node-inspector-title").textContent,
    subtitle: document.querySelector("#node-inspector-subtitle").textContent,
    state: document.querySelector("#node-inspector-state").textContent,
    action: document.querySelector("#node-inspector-action-button").textContent,
    portRows: document.querySelectorAll("#node-inspector-port-list .node-inspector-port-row").length,
  }));
  assert.deepEqual(outputDialog, {
    open: true,
    title: "LSL recorder",
    subtitle: "Output",
    state: "Awaiting input",
    action: "Enable",
    portRows: 1,
  }, "clicking an output node did not open the node inspector popup");
  await page.locator("#node-inspector-close").click();
  await page.locator("#node-add-visualizer-button").click();
  const visualizerMenu = await page.evaluate(() => ({
    entries: [...document.querySelectorAll("#node-menu-list button strong")].map((node) => node.textContent),
    widgetCount: document.querySelectorAll("#node-menu-list button .node-menu-widget").length,
    codeBadgeCount: document.querySelectorAll("#node-menu-list button code").length,
  }));
  assert.deepEqual(visualizerMenu, {
    entries: ["ECG visualizer", "ACC visualizer", "Breathing visualizer"],
    widgetCount: 3,
    codeBadgeCount: 0,
  }, "visualizer node menu did not render every visualizer option with a colored SVG widget");
  await page.locator("#node-menu-list button", { hasText: "ACC visualizer" }).click();
  await page.locator(".patch-node", { hasText: "ACC visualizer" }).locator("p").click();
  const visualizerDialog = await page.evaluate(() => ({
    open: document.querySelector("#node-inspector-dialog").open,
    title: document.querySelector("#node-inspector-title").textContent,
    subtitle: document.querySelector("#node-inspector-subtitle").textContent,
    state: document.querySelector("#node-inspector-state").textContent,
    action: document.querySelector("#node-inspector-action-button").textContent,
    portRows: document.querySelectorAll("#node-inspector-port-list .node-inspector-port-row").length,
  }));
  assert.deepEqual(visualizerDialog, {
    open: true,
    title: "ACC visualizer",
    subtitle: "Visualizer",
    state: "Awaiting input",
    action: "Open Visual",
    portRows: 1,
  }, "clicking a visualizer node did not open the node inspector popup");
  await page.locator("#node-inspector-close").click();
  await page.locator("#panel-view-toggle").click();
  const panelToggle = await page.evaluate(() => ({
    mode: document.body.dataset.viewMode,
    panelsHidden: document.querySelector("#workspace").hidden,
    nodesHidden: document.querySelector("#node-workspace").hidden,
    nodePressed: document.querySelector("#node-view-toggle").getAttribute("aria-pressed"),
    panelPressed: document.querySelector("#panel-view-toggle").getAttribute("aria-pressed"),
  }));
  assert.deepEqual(panelToggle, {
    mode: "panels",
    panelsHidden: false,
    nodesHidden: true,
    nodePressed: "false",
    panelPressed: "true",
  }, "panel view toggle did not restore the three-panel workspace");
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
  assert.equal(await page.getByLabel("Volume algorithm").count(), 0, "new settings must not offer the legacy volume algorithm");
  assert.equal(await page.getByLabel("Phase algorithm").count(), 0, "new settings must not offer the legacy phase algorithm");
  assert.match(await page.locator("#module-settings").textContent(), /Release processor: Timed PCA v1/);
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
  const stackedColors = await inspectCanvasTraceColors(page, [[19, 104, 170]]);
  assert.ok(stackedColors[0] > 120, `the blue ACC traces were not drawn: ${JSON.stringify(stackedColors)}`);
  const accelerometerScreenshot = join(output, "raw-accelerometer-stacked.png");
  await page.screenshot({ path: accelerometerScreenshot, fullPage: true });
  assert.ok((await stat(accelerometerScreenshot)).size > 20_000, "stacked ACC screenshot was unexpectedly empty");

  const multipleSources = await page.evaluate(() => window.PolarInterfaceRenderer.render("multiple-colored-sources"));
  assert.deepEqual(multipleSources.sourceOptions, ["source-1", "source-2", "source-3"]);
  assert.deepEqual(multipleSources.chipColors, ["#1368AA", "#B43C4C", "#18794E"]);
  assert.deepEqual(multipleSources.connectedWidgets, [
    {
      sourceId: "source-1", profile: "polar", color: "#1368AA", cardiacColor: "#1368AA",
      breathingColor: "#1368AA", swatchCount: 1,
      pickerLabel: "Source color for Polar H10 A", hasKeepConnected: false, keepConnected: null,
    },
    {
      sourceId: "source-2", profile: "vernier", color: "#B43C4C", cardiacColor: "#B43C4C",
      breathingColor: "#B43C4C", swatchCount: 1,
      pickerLabel: "Source color for GDX-RB A", hasKeepConnected: false, keepConnected: null,
    },
    {
      sourceId: "source-3", profile: "polar", color: "#18794E", cardiacColor: "#18794E",
      breathingColor: "#18794E", swatchCount: 1,
      pickerLabel: "Source color for Polar H10 B", hasKeepConnected: false, keepConnected: null,
    },
  ]);
  assert.equal(multipleSources.palettePickerCount, 3);
  assert.ok(multipleSources.availableDeviceCount >= 1, "available devices disappeared after sources connected");
  assert.equal(multipleSources.selectedSource, "source-2");
  assert.equal(multipleSources.chartColor, "#B43C4C");
  assert.ok(multipleSources.outputColors.every((color) => color === "#B43C4C"));
  assert.notEqual(multipleSources.forceValue, "—");
  assert.match(multipleSources.breathingValue, /^(?:0\.\d{3}|1\.000)$/);
  assert.equal(multipleSources.selectedVisual, "vernier_breathing");
  assert.deepEqual(multipleSources.visualOptions, ["raw_force", "vernier_breathing"]);
  assert.deepEqual(multipleSources.comparisonOptions, ["source-1", "source-3"]);
  assert.deepEqual(multipleSources.comparisonSelected, []);
  assert.equal(multipleSources.comparisonHidden, false, "compatible breathing sources were not offered");
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
  await page.getByLabel("Source color for GDX-RB A").selectOption("lagoon");
  const recolored = await page.evaluate(() => ({
    widget: document.querySelector('[data-source-id="source-2"]').style.getPropertyValue("--source-color"),
    chart: document.querySelector("#chart-shell").style.getPropertyValue("--source-color"),
    outputs: [...document.querySelector("#output-chips").children].map((card) => card.style.getPropertyValue("--source-color")),
    outputPanelMarked: document.querySelector("#output-workspace").classList.contains("source-panel-marked"),
    visualPanelMarked: document.querySelector("#visual-workspace").classList.contains("source-panel-marked"),
  }));
  assert.equal(recolored.widget, "#007A78");
  assert.equal(recolored.chart, "#007A78");
  assert.ok(recolored.outputs.every((color) => color === "#007A78"));
  assert.equal(recolored.outputPanelMarked, true);
  assert.equal(recolored.visualPanelMarked, true);
  await page.screenshot({ path: join(output, "multiple-colored-sources.png"), fullPage: true });

  const breathingTraceColors = ["#1368AA", "#B43C4C", "#18794E"];
  const breathingTraceRgb = [[19, 104, 170], [180, 60, 76], [24, 121, 78]];
  const comparisonOverlay = await page.evaluate(() => window.PolarInterfaceRenderer.render("multi-source-comparison-overlay"));
  assert.equal(comparisonOverlay.selectedVisual, "breathing_volume");
  assert.ok(comparisonOverlay.visualOptions.includes("breathing_volume"));
  assert.deepEqual(comparisonOverlay.comparisonOptions, ["source-2", "source-3"]);
  assert.deepEqual(comparisonOverlay.comparisonSelected, ["source-2", "source-3"]);
  assert.equal(comparisonOverlay.comparisonCount, "2 selected");
  assert.equal(comparisonOverlay.selectAllChecked, true);
  assert.deepEqual(comparisonOverlay.incompatibleComparisonOptions, { raw_ecg: ["source-3"], raw_acc: ["source-3"] });
  assert.equal(comparisonOverlay.visualMode, "time-aligned-comparison");
  assert.equal(comparisonOverlay.composite, "breathing_waveform_01");
  assert.equal(comparisonOverlay.comparisonLayout, "overlay");
  assert.deepEqual(comparisonOverlay.comparisonSources, ["source-1", "source-2", "source-3"]);
  assert.deepEqual(comparisonOverlay.traceColors, breathingTraceColors);
  assert.deepEqual(comparisonOverlay.alignmentStatuses, ["manual", "reference", "manual"]);
  assert.deepEqual(comparisonOverlay.alignmentSigns, [-1, 1, -1]);
  assert.deepEqual(comparisonOverlay.breathingRawKinds, ["polar-projection", "vernier-force", "polar-projection"]);
  assert.deepEqual(comparisonOverlay.alignmentControls, [
    { sourceId: "source-1", value: "flip" },
    { sourceId: "source-3", value: "flip" },
  ]);
  assert.equal(comparisonOverlay.laneCount, 1);
  assert.ok(
    comparisonOverlay.smoothPlayheadAdvance >= 0.025,
    `breathing playhead did not advance between input packets: ${comparisonOverlay.smoothPlayheadAdvance}`,
  );
  assert.ok(
    comparisonOverlay.latestTracePoint <= comparisonOverlay.comparisonEnd + 1e-6,
    "breathing renderer extrapolated a trace beyond the delayed display playhead",
  );
  assert.deepEqual(comparisonOverlay.layoutInputs, { overlay: true, separate: false });
  assert.match(comparisonOverlay.currentLabel, /^Source 1 0\.\d{3} · Source 2 0\.\d{3} · Source 3 0\.\d{3}$/);
  assert.match(comparisonOverlay.canvasLabel, /comparative overlay view of 3 independent breathing sources/i);
  assert.match(comparisonOverlay.canvasLabel, /matched to Vernier automatically or manually/i);
  assert.match(comparisonOverlay.chartClass, /stacked-axes/);
  assert.doesNotMatch(comparisonOverlay.chartClass, /comparison-separate/);
  assert.deepEqual(comparisonOverlay.legendLabels, [
    "Source 1 · Polar H10 A", "Source 2 · GDX-RB A", "Source 3 · Polar H10 B",
  ]);
  assert.deepEqual(comparisonOverlay.legendColors, ["rgb(19, 104, 170)", "rgb(180, 60, 76)", "rgb(24, 121, 78)"]);
  const overlayColorCounts = await inspectCanvasTraceColors(page, breathingTraceRgb);
  assert.ok(overlayColorCounts.every((count) => count > 25), `one or more breathing overlays were not drawn: ${JSON.stringify(overlayColorCounts)}`);
  const comparisonCanvas = await inspectCanvas(page);
  assert.ok(comparisonCanvas.width > 500 && comparisonCanvas.height > 150, `time-aligned overlay did not span the canvas: ${JSON.stringify(comparisonCanvas)}`);
  const overlayScreenshot = join(output, "multi-source-comparison-overlay.png");
  await page.screenshot({ path: overlayScreenshot, fullPage: true });
  assert.ok((await stat(overlayScreenshot)).size > 20_000, "overlay comparison screenshot was unexpectedly empty");

  const comparisonSeparate = await page.evaluate(() => window.PolarInterfaceRenderer.render("multi-source-comparison-separate"));
  assert.equal(comparisonSeparate.visualMode, "time-aligned-comparison");
  assert.equal(comparisonSeparate.composite, "breathing_waveform_01");
  assert.equal(comparisonSeparate.comparisonLayout, "separate");
  assert.deepEqual(comparisonSeparate.comparisonSources, ["source-1", "source-2", "source-3"]);
  assert.deepEqual(comparisonSeparate.traceColors, breathingTraceColors);
  assert.deepEqual(comparisonSeparate.alignmentStatuses, comparisonOverlay.alignmentStatuses);
  assert.deepEqual(comparisonSeparate.alignmentSigns, comparisonOverlay.alignmentSigns);
  assert.deepEqual(comparisonSeparate.breathingRawKinds, comparisonOverlay.breathingRawKinds);
  assert.equal(comparisonSeparate.laneCount, 3);
  assert.deepEqual(comparisonSeparate.layoutInputs, { overlay: false, separate: true });
  assert.match(comparisonSeparate.canvasLabel, /comparative separate-lane view of 3 independent breathing sources/i);
  assert.match(comparisonSeparate.chartClass, /comparison-separate/);
  assert.deepEqual(comparisonSeparate.legendLabels, comparisonOverlay.legendLabels);
  const separateColorCounts = await inspectCanvasTraceColors(page, breathingTraceRgb);
  assert.ok(separateColorCounts.every((count) => count > 25), `one or more separate breathing lanes were not drawn: ${JSON.stringify(separateColorCounts)}`);
  const separateScreenshot = join(output, "multi-source-comparison-separate.png");
  await page.screenshot({ path: separateScreenshot, fullPage: true });
  assert.ok((await stat(separateScreenshot)).size > 20_000, "separate comparison screenshot was unexpectedly empty");

  const reconnectComparison = await page.evaluate(() => window.PolarInterfaceRenderer.probeComparisonReconnect());
  assert.equal(reconnectComparison.duringReconnect.selectedSourceId, "source-2");
  assert.deepEqual(reconnectComparison.restored, {
    selectedSourceId: "source-1",
    visibleSourceIds: ["source-1", "source-2", "source-3"],
  }, `comparison membership was not restored: ${JSON.stringify(reconnectComparison)}`);

  const defaultWorkspaceProportions = [0.84 / 3.16, 1 / 3.16, 1.32 / 3.16];
  const initialWorkspace = await inspectWorkspaceLayout(page);
  assert.ok(initialWorkspace.dividers.every((divider) => divider.display === "block" && divider.width === 7));
  assert.ok(initialWorkspace.dividers.every((divider) => divider.tabIndex === 0 && divider.ariaHidden === null));
  assert.deepEqual(initialWorkspace.dividers.map(({ role, orientation, controls }) => ({ role, orientation, controls })), [
    { role: "separator", orientation: "vertical", controls: "input-section output-section" },
    { role: "separator", orientation: "vertical", controls: "output-section visual-section" },
  ]);
  assert.ok(initialWorkspace.dividers.every((divider) => divider.hitWidth >= 11), "divider pointer target is too narrow");
  assert.ok(initialWorkspace.dividers.every((divider) => (
    Number.isFinite(divider.valueNow)
    && divider.valueNow >= divider.valueMin
    && divider.valueNow <= divider.valueMax
    && /Input \d+%, Output \d+%, Visualization \d+%/.test(divider.valueText)
  )), `workspace divider ARIA state is incomplete: ${JSON.stringify(initialWorkspace.dividers)}`);
  assert.ok(Math.abs(
    initialWorkspace.panelWidths.reduce((sum, width) => sum + width, 0)
      + initialWorkspace.dividerWidth - initialWorkspace.workspaceWidth,
  ) < 1.5, "desktop panes and dividers do not preserve total workspace width");

  await dragWorkspaceDivider(page, "#input-output-divider", 72);
  const afterFirstDivider = await inspectWorkspaceLayout(page);
  assert.ok(afterFirstDivider.panelWidths[0] - initialWorkspace.panelWidths[0] > 66, "first divider did not grow Input");
  assert.ok(initialWorkspace.panelWidths[1] - afterFirstDivider.panelWidths[1] > 66, "first divider did not shrink Output");
  assert.ok(Math.abs(afterFirstDivider.panelWidths[2] - initialWorkspace.panelWidths[2]) < 1.5,
    "first divider changed the non-adjacent Visualization pane");

  await dragWorkspaceDivider(page, "#output-visual-divider", -64);
  const afterSecondDivider = await inspectWorkspaceLayout(page);
  assert.ok(Math.abs(afterSecondDivider.panelWidths[0] - afterFirstDivider.panelWidths[0]) < 1.5,
    "second divider changed the non-adjacent Input pane");
  assert.ok(afterFirstDivider.panelWidths[1] - afterSecondDivider.panelWidths[1] > 58, "second divider did not shrink Output");
  assert.ok(afterSecondDivider.panelWidths[2] - afterFirstDivider.panelWidths[2] > 58, "second divider did not grow Visualization");
  assert.equal(afterSecondDivider.stored?.panes?.length, 3, "resized proportions were not persisted");
  assert.ok(Math.abs(afterSecondDivider.stored.panes.reduce((sum, pane) => sum + pane, 0) - 1) < 0.00001,
    "persisted pane proportions are not normalized");
  assert.ok(Math.abs(afterSecondDivider.canvasPixelWidth - afterSecondDivider.chartWidth * afterSecondDivider.pixelRatio) <= 2,
    "visualization canvas did not resize with its pane");
  const resizedWorkspaceScreenshot = join(output, "workspace-resized-three-source.png");
  await page.screenshot({ path: resizedWorkspaceScreenshot, fullPage: true });
  assert.ok((await stat(resizedWorkspaceScreenshot)).size > 20_000, "resized workspace screenshot was unexpectedly empty");

  const persistedWorkspaceProportions = [...afterSecondDivider.stored.panes];
  await page.reload({ waitUntil: "networkidle" });
  await page.waitForFunction(() => Boolean(window.PolarInterfaceRenderer));
  await page.evaluate(() => window.PolarInterfaceRenderer.ready());
  await settleWorkspaceLayout(page);
  const restoredWorkspace = await inspectWorkspaceLayout(page);
  restoredWorkspace.proportions.forEach((pane, index) => {
    assert.ok(Math.abs(pane - persistedWorkspaceProportions[index]) < 0.003,
      `pane ${index + 1} did not restore its persisted proportion`);
  });

  const firstDivider = page.locator("#input-output-divider");
  await firstDivider.dblclick();
  await settleWorkspaceLayout(page);
  await firstDivider.focus();
  const beforeArrow = await inspectWorkspaceLayout(page);
  await page.keyboard.press("ArrowRight");
  await settleWorkspaceLayout(page);
  const afterArrow = await inspectWorkspaceLayout(page);
  const arrowDelta = afterArrow.panelWidths[0] - beforeArrow.panelWidths[0];
  assert.ok(arrowDelta > 12 && arrowDelta < 20, `ArrowRight used an unexpected step: ${arrowDelta}`);
  await firstDivider.dblclick();
  await settleWorkspaceLayout(page);
  const beforeShiftArrow = await inspectWorkspaceLayout(page);
  await firstDivider.focus();
  await page.keyboard.press("Shift+ArrowRight");
  await settleWorkspaceLayout(page);
  const afterShiftArrow = await inspectWorkspaceLayout(page);
  const shiftArrowDelta = afterShiftArrow.panelWidths[0] - beforeShiftArrow.panelWidths[0];
  assert.ok(shiftArrowDelta > arrowDelta * 2.5, `Shift+ArrowRight did not use a larger step: ${shiftArrowDelta}`);
  await page.keyboard.press("Home");
  await settleWorkspaceLayout(page);
  const homeMinimumWorkspace = await inspectWorkspaceLayout(page);
  assert.equal(homeMinimumWorkspace.dividers[0].valueNow, homeMinimumWorkspace.dividers[0].valueMin,
    "Home did not move the focused separator to its minimum");
  assert.ok(Math.abs(homeMinimumWorkspace.panelWidths[2] - afterShiftArrow.panelWidths[2]) < 1.5,
    "Home changed the non-adjacent Visualization pane");
  await page.keyboard.press("End");
  await settleWorkspaceLayout(page);
  const endMaximumWorkspace = await inspectWorkspaceLayout(page);
  assert.equal(endMaximumWorkspace.dividers[0].valueNow, endMaximumWorkspace.dividers[0].valueMax,
    "End did not move the focused separator to its maximum");
  assert.ok(Math.abs(endMaximumWorkspace.panelWidths[2] - homeMinimumWorkspace.panelWidths[2]) < 1.5,
    "End changed the non-adjacent Visualization pane");

  await firstDivider.dblclick();
  await settleWorkspaceLayout(page);
  const doubleClickResetWorkspace = await inspectWorkspaceLayout(page);
  doubleClickResetWorkspace.proportions.forEach((pane, index) => {
    assert.ok(Math.abs(pane - defaultWorkspaceProportions[index]) < 0.003,
      `double-click did not reset pane ${index + 1}`);
  });
  assert.deepEqual(doubleClickResetWorkspace.stored?.panes, defaultWorkspaceProportions.map((pane) => Number(pane.toFixed(6))));

  await page.evaluate(() => window.PolarInterfaceRenderer.render("multi-source-comparison-separate"));
  await settleWorkspaceLayout(page);

  await page.setViewportSize({ width: 900, height: 780 });
  await settleWorkspaceLayout(page);
  const mobileWorkspace = await inspectWorkspaceLayout(page);
  assert.ok(mobileWorkspace.dividers.every((divider) => (
    divider.display === "none" && divider.tabIndex === -1 && divider.ariaHidden === "true" && divider.ariaDisabled === "true"
  )), "workspace dividers remain interactive in the single-column layout");
  assert.ok(mobileWorkspace.panelWidths.every((width) => Math.abs(width - mobileWorkspace.workspaceWidth) < 1.5),
    "mobile panels are not full-width single-column lanes");
  assert.ok(mobileWorkspace.panelTops[0] < mobileWorkspace.panelTops[1] && mobileWorkspace.panelTops[1] < mobileWorkspace.panelTops[2],
    "mobile panels are not vertically ordered");
  await page.setViewportSize({ width: 1440, height: 900 });
  await settleWorkspaceLayout(page);
  const desktopWorkspaceAgain = await inspectWorkspaceLayout(page);
  assert.ok(desktopWorkspaceAgain.dividers.every((divider) => divider.display === "block" && divider.tabIndex === 0));
  desktopWorkspaceAgain.proportions.forEach((pane, index) => {
    assert.ok(Math.abs(pane - defaultWorkspaceProportions[index]) < 0.003,
      `pane ${index + 1} did not restore after returning from mobile`);
  });

  await page.setViewportSize({ width: 901, height: 780 });
  await settleWorkspaceLayout(page);
  const narrowDesktopWorkspace = await inspectWorkspaceLayout(page);
  assert.ok(narrowDesktopWorkspace.dividers.every((divider) => (
    divider.display === "block"
    && divider.tabIndex === 0
    && divider.valueMin < divider.valueMax
    && divider.valueNow >= divider.valueMin
    && divider.valueNow <= divider.valueMax
  )), `901px divider constraints are not operable: ${JSON.stringify(narrowDesktopWorkspace.dividers)}`);
  const secondDivider = page.locator("#output-visual-divider");
  await secondDivider.focus();
  await page.keyboard.press("End");
  await settleWorkspaceLayout(page);
  const narrowVisualMinimum = await inspectWorkspaceLayout(page);
  assert.equal(narrowVisualMinimum.dividers[1].valueNow, narrowVisualMinimum.dividers[1].valueMax);
  assert.ok(narrowVisualMinimum.panelScrollWidths[2] <= narrowVisualMinimum.panelWidths[2] + 2,
    `narrow Visualization pane overflows horizontally: ${JSON.stringify(narrowVisualMinimum)}`);
  assert.ok(Math.abs(narrowVisualMinimum.canvasPixelWidth - narrowVisualMinimum.chartWidth * narrowVisualMinimum.pixelRatio) <= 2,
    "canvas backing width did not follow the narrow Visualization pane");
  await secondDivider.dblclick();
  await page.setViewportSize({ width: 1440, height: 900 });
  await settleWorkspaceLayout(page);

  const accLibrary = await page.evaluate(() => window.PolarInterfaceRenderer.render("acc-primary-library"));
  assert.deepEqual(accLibrary.primaryIds, [
    "raw_acc", "acc_magnitude", "breathing_volume",
    "breathing_signal_confidence", "breathing_signal_ready",
  ]);
  for (const compatibilityId of [
    "acc_breathing_magnitude", "breathing_phase", "breathing_calibration",
    "breathing_axis_range", "breathing_rate", "breathing_dynamics_confidence",
    "breath_interval_sampen", "breath_amplitude_mse",
  ]) {
    assert.ok(accLibrary.compatibilityIds.includes(compatibilityId), `${compatibilityId} lost compatibility coverage`);
    assert.ok(!accLibrary.primaryIds.includes(compatibilityId), `${compatibilityId} leaked into new selection`);
  }

  const restoredCompatibility = await page.evaluate(() => window.PolarInterfaceRenderer.render("compatibility-output-restore"));
  assert.equal(restoredCompatibility.before.restored, true);
  assert.match(restoredCompatibility.before.className, /compatibility-output-card/);
  assert.match(restoredCompatibility.before.note, /Compatibility only · restored legacy output/);
  assert.match(restoredCompatibility.before.settings, /Restored compatibility processor: Legacy v0/);
  assert.equal(restoredCompatibility.before.upgradeButton, true);
  assert.equal(restoredCompatibility.before.visibleInNewSelection, false);
  assert.equal(restoredCompatibility.presentAfterRemove, false);

  const restoredRelease = await page.evaluate(() => window.PolarInterfaceRenderer.render("release-breathing-compatibility"));
  const releaseBreathingIds = ["breathing_volume", "breathing_signal_confidence", "breathing_signal_ready"];
  const sortedReleaseBreathingIds = [...releaseBreathingIds].sort();
  const currentProcessorModes = Object.fromEntries(releaseBreathingIds.map((id) => [id, {
    volumeMode: "timed-pca-v1",
    stateMode: "hysteresis-v1",
  }]));
  assert.equal(restoredRelease.legacy.storedOptionsPreserved, true, "rendering rewrote restored legacy options");
  assert.deepEqual(restoredRelease.legacy.before.selectedIds, releaseBreathingIds);
  assert.deepEqual(restoredRelease.legacy.before.savedIds, releaseBreathingIds);
  assert.deepEqual(restoredRelease.legacy.before.allSavedIds, [...releaseBreathingIds, "breathing_phase"], "restore changed the saved release/legacy IDs");
  assert.deepEqual(restoredRelease.legacy.before.selectedCompatibilityIds, ["breathing_phase"]);
  assert.deepEqual(restoredRelease.legacy.before.phaseModes, {
    volumeMode: "legacy-v0",
    stateMode: "legacy-v0",
  });
  assert.deepEqual(restoredRelease.legacy.before.compatibilityIds, releaseBreathingIds);
  assert.ok(restoredRelease.legacy.before.notes.every((note) => /Compatibility only · restored Legacy v0 processor/.test(note)));
  assert.equal(restoredRelease.legacy.before.upgradeButtonCount, 1);
  assert.match(restoredRelease.legacy.module.text, /Restored compatibility processor: Legacy v0/);
  assert.equal(restoredRelease.legacy.module.upgradeButtonCount, 1);
  assert.equal(restoredRelease.legacy.before.storedVolumeNormalization, "slidingWindow");
  assert.equal(restoredRelease.legacy.before.effectiveVolumeNormalization, "none");
  assert.match(restoredRelease.legacy.before.volumeSummary, /canonical 0–1/);
  assert.deepEqual(restoredRelease.legacy.after.selectedIds, releaseBreathingIds);
  assert.deepEqual(restoredRelease.legacy.after.savedIds, releaseBreathingIds);
  assert.deepEqual(restoredRelease.legacy.after.compatibilityIds, []);
  assert.equal(restoredRelease.legacy.after.upgradeButtonCount, 0);
  assert.equal(restoredRelease.legacy.after.volumeMode, "timed-pca-v1");
  assert.equal(restoredRelease.legacy.after.stateMode, "hysteresis-v1");
  assert.equal(restoredRelease.legacy.after.storedVolumeNormalization, "none");
  assert.deepEqual(restoredRelease.legacy.after.selectedRespirationIds, sortedReleaseBreathingIds);
  assert.deepEqual(restoredRelease.legacy.after.selectedCompatibilityIds, [], "explicit upgrade retained legacy breathing_phase");
  assert.equal(restoredRelease.legacy.after.phaseModes, null, "explicit upgrade retained legacy breathing_phase options");
  assert.deepEqual(restoredRelease.legacy.after.configuredRespirationIds, sortedReleaseBreathingIds);
  assert.deepEqual(restoredRelease.legacy.after.configuredModes, currentProcessorModes);

  assert.equal(restoredRelease.partial.storedOptionsPreserved, true, "rendering rewrote restored partial options");
  assert.deepEqual(restoredRelease.partial.before.selectedIds, ["breathing_volume", "breathing_signal_ready"]);
  assert.deepEqual(restoredRelease.partial.before.savedIds, ["breathing_volume", "breathing_signal_ready"]);
  assert.deepEqual(restoredRelease.partial.before.allSavedIds, ["breathing_volume", "breathing_signal_ready"], "restore completed a partial set without consent");
  assert.deepEqual(restoredRelease.partial.before.compatibilityIds, ["breathing_volume", "breathing_signal_ready"]);
  assert.ok(restoredRelease.partial.before.notes.every((note) => /Compatibility only · incomplete waveform \+ quality set/.test(note)));
  assert.equal(restoredRelease.partial.before.upgradeButtonCount, 1);
  assert.deepEqual(restoredRelease.partial.after.selectedIds, releaseBreathingIds);
  assert.deepEqual(restoredRelease.partial.after.savedIds, releaseBreathingIds);
  assert.deepEqual(restoredRelease.partial.after.compatibilityIds, []);
  assert.equal(restoredRelease.partial.after.upgradeButtonCount, 0);
  assert.equal(restoredRelease.partial.after.volumeMode, "timed-pca-v1");
  assert.equal(restoredRelease.partial.after.stateMode, "hysteresis-v1");
  assert.deepEqual(restoredRelease.partial.after.selectedRespirationIds, sortedReleaseBreathingIds);
  assert.deepEqual(restoredRelease.partial.after.configuredRespirationIds, sortedReleaseBreathingIds);
  assert.deepEqual(restoredRelease.partial.after.configuredModes, currentProcessorModes);

  const transactionBaseline = await page.evaluate(() => window.PolarInterfaceRenderer.render("output-config-transaction-baseline"));
  const transactionSourceIds = [...transactionBaseline.activeSourceIds].sort();
  assert.deepEqual(transactionBaseline.outputs.sort(), ["raw_acc", "raw_ecg"]);
  assert.deepEqual(transactionBaseline.savedOutputIds.sort(), ["raw_acc", "raw_ecg"]);
  assert.ok(transactionSourceIds.length >= 2, "transaction test lost the simultaneous Polar/Vernier source setup");

  await page.evaluate(() => {
    window.AudioContext = class FakeAudioContext {
      constructor() {
        this.sampleRate = 44_100;
        this.currentTime = 0;
        this.destination = {};
        this.state = "suspended";
      }

      async resume() { this.state = "running"; }
      async suspend() { this.state = "suspended"; }
      createGain() {
        return { gain: { value: 0 }, connect() {}, disconnect() {} };
      }
    };
  });
  await page.evaluate(() => window.PolarInterfaceRenderer.rejectNextOutputConfig("Injected audio enable rejection"));
  await page.locator("#audio-toggle").evaluate((input) => {
    input.checked = true;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  const rejectedAudioEnable = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(await page.locator("#audio-toggle").isChecked(), false, "failed audio enable left its toggle on");
  assert.equal(await page.evaluate(() => window.PolarAudioDataLink.status().enabled), false, "failed audio enable left the modem running");
  assert.equal(rejectedAudioEnable.config.audioEnabled, false, "failed audio enable changed the saved configuration");
  assert.ok(rejectedAudioEnable.toastMessages.some((toast) => toast.error && /Injected audio enable rejection/.test(toast.message)));
  assert.ok(!rejectedAudioEnable.toastMessages.some((toast) => /modem active/.test(toast.message)), "failed audio enable announced success");

  await page.evaluate(() => document.querySelector("#toast-region").replaceChildren());
  await page.locator("#audio-toggle").evaluate((input) => {
    input.checked = true;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  const acceptedAudioEnable = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(await page.evaluate(() => window.PolarAudioDataLink.status().enabled), true, "accepted audio enable did not start the modem");
  assert.equal(acceptedAudioEnable.config.audioEnabled, true);
  await page.evaluate(() => document.querySelector("#toast-region").replaceChildren());
  await page.evaluate(() => window.PolarInterfaceRenderer.rejectNextOutputConfig("Injected audio disable rejection"));
  await page.locator("#audio-toggle").evaluate((input) => {
    input.checked = false;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  const rejectedAudioDisable = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(await page.locator("#audio-toggle").isChecked(), true, "failed audio disable did not restore its toggle");
  assert.equal(await page.evaluate(() => window.PolarAudioDataLink.status().enabled), true, "failed audio disable stopped the committed modem");
  assert.equal(rejectedAudioDisable.config.audioEnabled, true, "failed audio disable changed the saved configuration");
  assert.ok(rejectedAudioDisable.toastMessages.some((toast) => toast.error && /Injected audio disable rejection/.test(toast.message)));
  await page.locator("#audio-toggle").evaluate((input) => {
    input.checked = false;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(await page.evaluate(() => window.PolarAudioDataLink.status().enabled), false, "audio cleanup did not stop the modem");

  await page.locator("#open-output-dialog").click();
  await page.getByRole("button", { name: /ACC metrics/ }).click();
  await page.locator('.metric-option[data-metric-id="breathing_volume"]').click();
  await page.evaluate(() => window.PolarInterfaceRenderer.rejectNextOutputConfig("Injected add rejection"));
  await page.locator("#save-metric-output").click();
  const rejectedAdd = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(rejectedAdd.outputDialogOpen, true, "failed add closed the metric library");
  assert.deepEqual(rejectedAdd.outputs.sort(), ["raw_acc", "raw_ecg"], "failed add remained visible");
  assert.deepEqual(rejectedAdd.savedOutputIds.sort(), ["raw_acc", "raw_ecg"], "failed add remained saved");
  assert.deepEqual([...rejectedAdd.config.outputs].sort(), ["raw_acc", "raw_ecg"], "failed add changed the saved configuration");
  assert.deepEqual([...rejectedAdd.activeSourceIds].sort(), transactionSourceIds, "failed add disturbed connected sources");
  assert.ok(rejectedAdd.toastMessages.some((toast) => toast.error && /Injected add rejection/.test(toast.message)));
  assert.ok(!rejectedAdd.toastMessages.some((toast) => /added together/.test(toast.message)), "failed add announced success");

  await page.locator("#save-metric-output").click();
  const acceptedAdd = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(acceptedAdd.outputDialogOpen, false);
  assert.deepEqual(
    releaseBreathingIds.filter((id) => acceptedAdd.outputs.includes(id)),
    releaseBreathingIds,
    "successful retry did not apply the complete release set",
  );

  await page.evaluate(() => document.querySelector("#toast-region").replaceChildren());
  await page.evaluate(() => window.PolarInterfaceRenderer.rejectNextOutputConfig("Injected remove rejection"));
  await page.locator('.output-card[data-metric-id="breathing_volume"] button[aria-label^="Remove "]').click();
  const rejectedRemove = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.deepEqual(
    releaseBreathingIds.filter((id) => rejectedRemove.outputs.includes(id)),
    releaseBreathingIds,
    "failed grouped removal was not restored",
  );
  assert.deepEqual([...rejectedRemove.activeSourceIds].sort(), transactionSourceIds, "failed removal disturbed connected sources");
  assert.ok(rejectedRemove.toastMessages.some((toast) => toast.error && /Injected remove rejection/.test(toast.message)));
  assert.ok(!rejectedRemove.toastMessages.some((toast) => /removed together/.test(toast.message)), "failed removal announced success");

  await page.locator('.output-card[data-metric-id="breathing_volume"] .module-tune-button').click();
  const originalDisplayWindow = (await page.evaluate(() => window.PolarInterfaceRenderer.metricOptions("breathing_volume"))).displayWindowSeconds;
  await page.locator('#module-settings input[type="number"]').first().fill(String(originalDisplayWindow + 2));
  await page.evaluate(() => document.querySelector("#toast-region").replaceChildren());
  await page.evaluate(() => window.PolarInterfaceRenderer.rejectNextOutputConfig("Injected settings rejection"));
  await page.locator("#save-module-settings").click();
  const rejectedSettings = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(rejectedSettings.moduleDialogOpen, true, "failed settings save closed the module dialog");
  assert.equal(
    (await page.evaluate(() => window.PolarInterfaceRenderer.metricOptions("breathing_volume"))).displayWindowSeconds,
    originalDisplayWindow,
    "failed settings save remained applied",
  );
  assert.equal(
    rejectedSettings.config.metricOptions.breathing_volume.displayWindowSeconds,
    originalDisplayWindow,
    "failed settings save changed the saved configuration",
  );
  assert.ok(rejectedSettings.toastMessages.some((toast) => toast.error && /Injected settings rejection/.test(toast.message)));
  assert.ok(!rejectedSettings.toastMessages.some((toast) => /settings saved/.test(toast.message)), "failed settings save announced success");

  const transactionLegacy = await page.evaluate(() => window.PolarInterfaceRenderer.render("output-config-transaction-legacy"));
  assert.equal(transactionLegacy.moduleDialogOpen, true);
  assert.equal(transactionLegacy.breathingSettings.volumeMode, "legacy-v0");
  await page.evaluate(() => window.PolarInterfaceRenderer.rejectNextOutputConfig("Injected upgrade rejection"));
  await page.locator('#module-settings [data-action="upgrade-polar-respiration"]').click();
  const rejectedUpgrade = await page.evaluate(() => window.PolarInterfaceRenderer.waitForOutputConfig());
  assert.equal(rejectedUpgrade.moduleDialogOpen, true, "failed upgrade closed the module dialog");
  assert.equal(rejectedUpgrade.breathingSettings.volumeMode, "legacy-v0", "failed upgrade changed the processor mode");
  assert.equal(rejectedUpgrade.breathingSettings.stateMode, "legacy-v0", "failed upgrade changed the state mode");
  assert.equal(rejectedUpgrade.config.metricOptions.breathing_volume.processing.breathing.volumeMode, "legacy-v0");
  assert.equal(rejectedUpgrade.config.metricOptions.breathing_volume.processing.breathing.stateMode, "legacy-v0");
  assert.deepEqual([...rejectedUpgrade.activeSourceIds].sort(), transactionSourceIds, "failed upgrade disturbed connected sources");
  assert.ok(rejectedUpgrade.toastMessages.some((toast) => toast.error && /Injected upgrade rejection/.test(toast.message)));
  assert.ok(!rejectedUpgrade.toastMessages.some((toast) => /upgraded to/.test(toast.message)), "failed upgrade announced success");
  await page.locator("#module-dialog").evaluate((dialog) => dialog.close());

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
  assert.deepEqual(primaryAccIds, [
    "raw_acc", "acc_magnitude", "breathing_volume",
    "breathing_signal_confidence", "breathing_signal_ready",
  ]);
  assert.equal(await page.locator("#acc-extra-toggle").isHidden(), true);
  assert.ok(!primaryAccIds.includes("breathing_phase") && !primaryAccIds.includes("breathing_rate"));
  assert.match(await page.locator("#metric-library-summary").textContent(), /^5 of 5 ACC metrics$/);

  for (const metricId of ["breathing_volume", "breathing_signal_confidence", "breathing_signal_ready"]) {
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
  for (const [theme, expectedColor] of [["light", "#1368AA"], ["dark", "#67B7F7"]]) {
    const wantsDark = theme === "dark";
    const isDark = await page.locator("html").getAttribute("data-theme") === "dark";
    if (isDark !== wantsDark) await page.locator("#theme-toggle").click();
    assert.equal(await page.locator("#theme-toggle").getAttribute("aria-pressed"), String(wantsDark));
    assert.equal(await page.locator("meta[name='theme-color']").getAttribute("content"), wantsDark ? "#202428" : "#17221d");
    await page.evaluate(() => window.PolarInterfaceRenderer.render("multiple-colored-sources"));
    await page.locator("#visual-device").selectOption("source-1");
    assert.equal(await page.locator("#chart-shell").evaluate((node) => node.style.getPropertyValue("--source-color")), expectedColor);
    for (const selector of [
      '#connected-device-list [data-source-id="source-1"] .device-icon',
      '#connected-device-list [data-source-id="source-2"] .device-icon',
      ".device-profile-mark",
      ".activity-list time",
      ".osc-mark",
      ".destination-row strong em",
    ]) {
      await assertTextContrast(page, selector);
    }
    await page.evaluate(() => document.querySelector('[data-source-id="source-2"] button').click());
    await assertTextContrast(page, ".device-profile-mark");
    await page.evaluate(() => document.querySelector('[data-source-id="source-1"] button').click());
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width === 1440 ? 900 : 780 });
      const screenshot = join(output, `${theme}-${width}.png`);
      await page.screenshot({ path: screenshot, fullPage: true });
      assert.ok((await stat(screenshot)).size > 15_000, `${theme} ${width}px screenshot was unexpectedly empty`);
    }
  }

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => window.PolarInterfaceRenderer.render("metric-library-previews"));
  for (const selector of [
    ".metric-search input",
    ".metric-family-heading strong",
    ".family-choice.active",
    ".metric-option",
    ".metric-option > span:first-child",
    ".metric-detail section p",
    ".metric-source-list a",
    ".output-dialog .primary-button",
  ]) {
    await assertTextContrast(page, selector);
  }
  const darkLibraryScreenshot = join(output, "dark-output-library.png");
  await page.screenshot({ path: darkLibraryScreenshot, fullPage: true });
  assert.ok((await stat(darkLibraryScreenshot)).size > 20_000, "dark output library screenshot was unexpectedly empty");
  await page.locator("#output-dialog").evaluate((dialog) => dialog.close());

  await page.evaluate(() => window.PolarInterfaceRenderer.render("breathing-phase-settings"));
  for (const selector of [
    ".settings-section > p",
    ".setting-check",
    ".setting-field input",
    ".module-dialog .primary-button",
  ]) {
    await assertTextContrast(page, selector);
  }
  const darkSettingsScreenshot = join(output, "dark-module-settings.png");
  await page.screenshot({ path: darkSettingsScreenshot, fullPage: true });
  assert.ok((await stat(darkSettingsScreenshot)).size > 20_000, "dark module settings screenshot was unexpectedly empty");
  await page.locator("#module-dialog").evaluate((dialog) => dialog.close());

  await page.evaluate(() => window.PolarInterfaceRenderer.render("multiple-colored-sources"));
  for (const width of [1024, 940, 820]) {
    await page.setViewportSize({ width, height: 780 });
    const layout = await page.evaluate(() => ({
      viewportWidth: window.innerWidth,
      documentWidth: document.documentElement.scrollWidth,
      footerPosition: getComputedStyle(document.querySelector(".status-bar")).position,
    }));
    assert.ok(layout.documentWidth <= layout.viewportWidth, `${width}px layout has horizontal overflow`);
    if (width <= 960) assert.equal(layout.footerPosition, "static", `${width}px footer can obscure workspace content`);
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

  process.stdout.write(`Validated the quiet research-workbench UI, output transactions, draggable persisted desktop splitters, three-source breathing comparison, source palettes, light/dark desktop, dialog, mobile, and intermediate-width states, ${targets.length} classifier renders, and ${library.previewCount} metric previews in ${output}\n`);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
