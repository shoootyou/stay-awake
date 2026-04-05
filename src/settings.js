// Settings window — Tauri IPC bridge
const { invoke } = window.__TAURI__.core;

let originalConfig = {};

async function loadConfig() {
  try {
    const cfg = await invoke("get_config");
    originalConfig = cfg;

    document.getElementById("jiggle-mode").value = cfg.jiggle_mode;
    document.getElementById("interval").value = cfg.interval_secs;
    document.getElementById("interval-value").textContent = cfg.interval_secs + "s";
    document.getElementById("app-mode").value = cfg.mode;
    document.getElementById("autostart").checked = cfg.autostart;
    document.getElementById("language").value = cfg.language;
    document.getElementById("hotkey").textContent = cfg.global_hotkey;

    await checkAccessibility();
  } catch (e) {
    console.error("Failed to load config:", e);
  }
}

async function checkAccessibility() {
  try {
    const granted = await invoke("check_accessibility");
    const mode = document.getElementById("jiggle-mode").value;
    const needsMouse = mode !== "PowerOnly";
    const banner = document.getElementById("accessibility-banner");
    if (!granted && needsMouse) {
      banner.classList.remove("hidden");
    } else {
      banner.classList.add("hidden");
    }
  } catch (_) {
    // Non-macOS or unavailable — hide banner
    document.getElementById("accessibility-banner").classList.add("hidden");
  }
}

async function saveConfig() {
  const cfg = {
    jiggle_mode: document.getElementById("jiggle-mode").value,
    interval_secs: parseInt(document.getElementById("interval").value, 10),
    mode: document.getElementById("app-mode").value,
    autostart: document.getElementById("autostart").checked,
    language: document.getElementById("language").value,
    global_hotkey: originalConfig.global_hotkey,
  };

  try {
    await invoke("save_config", { newConfig: cfg });
    await invoke("close_settings_window");
  } catch (e) {
    console.error("Failed to save config:", e);
  }
}

async function cancel() {
  try {
    await invoke("close_settings_window");
  } catch (_) {}
}

async function grantAccessibility() {
  try {
    await invoke("request_accessibility");
  } catch (e) {
    console.error("Failed to request accessibility:", e);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  loadConfig();

  document.getElementById("interval").addEventListener("input", (e) => {
    document.getElementById("interval-value").textContent = e.target.value + "s";
  });

  document.getElementById("jiggle-mode").addEventListener("change", checkAccessibility);
  document.getElementById("save-btn").addEventListener("click", saveConfig);
  document.getElementById("cancel-btn").addEventListener("click", cancel);
  document.getElementById("grant-btn").addEventListener("click", grantAccessibility);
});
