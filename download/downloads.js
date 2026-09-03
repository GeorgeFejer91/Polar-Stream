(() => {
  "use strict";
  const agent = navigator.userAgent.toLowerCase();
  const platform = agent.includes("win") ? "windows" : agent.includes("mac") ? "macos" : agent.includes("linux") ? "linux" : null;
  if (!platform) return;
  document.querySelector(`[data-platform="${platform}"]`)?.classList.add("recommended");
  const labels = { windows: "Windows releases", macos: "macOS releases", linux: "Linux releases" };
  document.getElementById("recommended-download").textContent = `Browse ${labels[platform]}`;
  document.getElementById("platform-note").textContent = `Detected ${platform === "macos" ? "macOS" : platform[0].toUpperCase() + platform.slice(1)} · release candidates are listed separately from latest stable.`;
})();
