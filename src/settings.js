// Settings window — Tauri IPC bridge
const { invoke } = window.__TAURI__.core;

let originalConfig = {};

// Translate all elements with data-i18n attributes.
async function applyTranslations() {
  const elements = document.querySelectorAll("[data-i18n]");
  for (const el of elements) {
    const key = el.getAttribute("data-i18n");
    try {
      const text = await invoke("get_translation", { key });
      if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") {
        el.placeholder = text;
      } else {
        el.textContent = text;
      }
    } catch (_) {
      // Keep original text on failure.
    }
  }
}

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

    // Schedule fields
    document.getElementById("schedule-enabled").checked = cfg.schedule_enabled || false;
    const startH = String(cfg.schedule_start_hour || 9).padStart(2, "0");
    const startM = String(cfg.schedule_start_minute || 0).padStart(2, "0");
    document.getElementById("schedule-start").value = startH + ":" + startM;
    const endH = String(cfg.schedule_end_hour || 17).padStart(2, "0");
    const endM = String(cfg.schedule_end_minute || 0).padStart(2, "0");
    document.getElementById("schedule-end").value = endH + ":" + endM;

    const days = cfg.schedule_days || ["mon", "tue", "wed", "thu", "fri"];
    document.querySelectorAll(".day-checkboxes input").forEach((cb) => {
      cb.checked = days.includes(cb.value);
    });

    await loadProfiles(cfg.active_profile);
    await checkAccessibility();
    await applyTranslations();
  } catch (e) {
    console.error("Failed to load config:", e);
  }
}

async function loadProfiles(activeProfile) {
  try {
    const profiles = await invoke("list_profiles");
    const select = document.getElementById("profile-select");
    // Keep only the Default option
    select.innerHTML = "";
    const defaultOpt = document.createElement("option");
    defaultOpt.value = "Default";
    defaultOpt.textContent = "Default";
    select.appendChild(defaultOpt);

    for (const p of profiles) {
      const opt = document.createElement("option");
      opt.value = p.name;
      opt.textContent = p.name;
      select.appendChild(opt);
    }
    select.value = activeProfile || "Default";
  } catch (_) {
    // Profiles not available.
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
    document.getElementById("accessibility-banner").classList.add("hidden");
  }
}

async function autoSave() {
  const startParts = document.getElementById("schedule-start").value.split(":");
  const endParts = document.getElementById("schedule-end").value.split(":");

  const scheduleDays = [];
  document.querySelectorAll(".day-checkboxes input:checked").forEach((cb) => {
    scheduleDays.push(cb.value);
  });

  const cfg = {
    jiggle_mode: document.getElementById("jiggle-mode").value,
    interval_secs: parseInt(document.getElementById("interval").value, 10),
    mode: document.getElementById("app-mode").value,
    autostart: document.getElementById("autostart").checked,
    language: document.getElementById("language").value,
    global_hotkey: originalConfig.global_hotkey,
    schedule_enabled: document.getElementById("schedule-enabled").checked,
    schedule_start_hour: parseInt(startParts[0], 10),
    schedule_start_minute: parseInt(startParts[1], 10),
    schedule_end_hour: parseInt(endParts[0], 10),
    schedule_end_minute: parseInt(endParts[1], 10),
    schedule_days: scheduleDays,
    profiles: originalConfig.profiles || [],
    active_profile: document.getElementById("profile-select").value || "Default",
  };

  try {
    await invoke("save_config", { newConfig: cfg });

    const newLang = cfg.language;
    if (newLang !== originalConfig.language) {
      await invoke("set_language", { language: newLang });
      await applyTranslations();
      originalConfig.language = newLang;
    }
  } catch (e) {
    console.error("Failed to auto-save config:", e);
  }
}

async function grantAccessibility() {
  try {
    await invoke("request_accessibility");
  } catch (e) {
    console.error("Failed to request accessibility:", e);
  }
}

async function saveProfileAs() {
  const name = prompt("Profile name:");
  if (!name || !name.trim()) return;
  try {
    await invoke("save_profile", { name: name.trim() });
    await loadProfiles(name.trim());
  } catch (e) {
    console.error("Failed to save profile:", e);
  }
}

async function deleteProfile() {
  const select = document.getElementById("profile-select");
  const name = select.value;
  if (name === "Default") return;
  try {
    await invoke("delete_profile", { name });
    await loadProfiles("Default");
  } catch (e) {
    console.error("Failed to delete profile:", e);
  }
}

async function onProfileChange() {
  const select = document.getElementById("profile-select");
  const name = select.value;
  if (name === "Default") return;
  try {
    await invoke("load_profile", { name });
    await loadConfig();
  } catch (e) {
    console.error("Failed to load profile:", e);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  loadConfig();

  document.getElementById("interval").addEventListener("input", (e) => {
    document.getElementById("interval-value").textContent = e.target.value + "s";
  });

  document.getElementById("jiggle-mode").addEventListener("change", () => {
    checkAccessibility();
    autoSave();
  });
  document.getElementById("interval").addEventListener("change", autoSave);
  document.getElementById("app-mode").addEventListener("change", autoSave);
  document.getElementById("autostart").addEventListener("change", autoSave);
  document.getElementById("language").addEventListener("change", autoSave);
  document.getElementById("schedule-enabled").addEventListener("change", autoSave);
  document.getElementById("schedule-start").addEventListener("change", autoSave);
  document.getElementById("schedule-end").addEventListener("change", autoSave);
  document.querySelectorAll(".day-checkboxes input").forEach((cb) => {
    cb.addEventListener("change", autoSave);
  });

  document.getElementById("mode-info-btn").addEventListener("click", () => {
    const panel = document.getElementById("mode-info-panel");
    panel.classList.toggle("hidden");
  });

  document.getElementById("grant-btn").addEventListener("click", grantAccessibility);
  document.getElementById("profile-save-btn").addEventListener("click", saveProfileAs);
  document.getElementById("profile-delete-btn").addEventListener("click", deleteProfile);
  document.getElementById("profile-select").addEventListener("change", onProfileChange);
});
