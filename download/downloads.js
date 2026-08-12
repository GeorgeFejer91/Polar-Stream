(() => {
  "use strict";
  const agent = navigator.userAgent.toLowerCase();
  const platform = agent.includes("win") ? "windows" : agent.includes("mac") ? "macos" : agent.includes("linux") ? "linux" : null;
  if (!platform) return;
  document.querySelector(`[data-platform="${platform}"]`)?.classList.add("recommended");
  const labels = { windows: "Windows packages", macos: "macOS universal DMG", linux: "Linux packages" };
  document.getElementById("recommended-download").textContent = `Open ${labels[platform]}`;
  document.getElementById("platform-note").textContent = `Detected ${platform === "macos" ? "macOS" : platform[0].toUpperCase() + platform.slice(1)} · GitHub sign-in is required.`;
})();
