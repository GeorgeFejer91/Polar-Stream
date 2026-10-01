(() => {
  "use strict";
  const key = "vernier-stream-mini.theme.v1";
  let theme = null;
  try {
    const stored = window.localStorage.getItem(key);
    if (stored === "light" || stored === "dark") theme = stored;
  } catch (_error) {
    // Hardened WebViews may disable storage.
  }
  if (!theme) {
    theme = window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
  window.StreamMiniTheme = Object.freeze({ key, initial: theme });
})();
