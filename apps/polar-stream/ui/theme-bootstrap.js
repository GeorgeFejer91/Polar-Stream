(() => {
  "use strict";
  const STORAGE_KEY = "polar-stream.theme.v1";
  let theme = null;
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark") theme = stored;
  } catch (_error) {
    // Storage can be unavailable in hardened WebViews.
  }
  if (!theme) {
    theme = window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", theme === "dark" ? "#202428" : "#17221d");
  window.PolarTheme = Object.freeze({ STORAGE_KEY, initial: theme });
})();
