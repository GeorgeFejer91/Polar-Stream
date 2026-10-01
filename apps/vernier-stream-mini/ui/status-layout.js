import { measureLineStats, prepareWithSegments, setLocale } from "./vendor/pretext/layout.js";

const targets = "#connection-feedback, #device-scan-status, #bluetooth-state, #metric-count, .discovered-device strong, .discovered-device span, .metric-options strong, .metric-options span";

function measure(element) {
  const style = getComputedStyle(element);
  const width = element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  const lineHeight = parseFloat(style.lineHeight);
  if (width <= 0 || !Number.isFinite(lineHeight)) return;
  const font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const prepared = prepareWithSegments(element.textContent || "", font, {
    letterSpacing: Number.parseFloat(style.letterSpacing) || 0,
  });
  const { lineCount, maxLineWidth } = measureLineStats(prepared, width);
  const maxHeight = Number.parseFloat(style.maxHeight);
  const needsScroll = Number.isFinite(maxHeight) && lineCount * lineHeight > maxHeight;
  element.dataset.textFit = needsScroll ? "scroll" : maxLineWidth > width + 1 ? "reflow" : "fit";
  if (element.id === "connection-feedback") element.tabIndex = needsScroll ? 0 : -1;
}

document.fonts.ready.then(() => {
  setLocale(document.documentElement.lang || "en");
  let pending = false;
  const schedule = () => {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      document.querySelectorAll(targets).forEach(measure);
    });
  };
  new MutationObserver(schedule).observe(document.body, { childList: true, characterData: true, subtree: true });
  new ResizeObserver(schedule).observe(document.documentElement);
  schedule();
});
