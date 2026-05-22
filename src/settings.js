// Settings window — Tauri IPC bridge
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let originalConfig = {};

const isMac = navigator.platform.toUpperCase().indexOf("MAC") >= 0;

function formatHotkeyDisplay(hotkey) {
  return hotkey
    .replace("CmdOrCtrl", isMac ? "\u2318 Cmd" : "Ctrl")
    .replace("Shift", "\u21e7 Shift")
    .replace("Alt", "\u2325 Alt");
}

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
    // Scheduled is no longer in the UI select — fall back to Manual for old configs.
    const supportedModes = ["Manual", "AlwaysOn", "WiFi"];
    const modeValue = supportedModes.includes(cfg.mode) ? cfg.mode : "Manual";
    document.getElementById("app-mode").value = modeValue;
    updateModeDescription();
    document.getElementById("autostart").checked = cfg.autostart;
    document.getElementById("language").value = cfg.language;
    document.getElementById("hotkey").textContent = formatHotkeyDisplay(cfg.global_hotkey);

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

    // WiFi section — visible only when mode is WiFi
    const isWifiMode = modeValue === "WiFi";
    document.getElementById("wifi-section").style.display = isWifiMode ? "" : "none";
    if (isWifiMode) {
      await loadWifiState(cfg);
    }

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

const MODE_DESCRIPTIONS = {
  Manual:   "Engine starts and stops manually via the tray toggle or global hotkey.",
  AlwaysOn: "Engine runs continuously while Stay Awake is open.",
  WiFi:     "Engine activates automatically on registered networks. Requires Location Services.",
};

function updateModeDescription() {
  const select = document.getElementById("app-mode");
  const desc = document.getElementById("app-mode-desc");
  if (!select || !desc) return;
  desc.textContent = MODE_DESCRIPTIONS[select.value] || "";
}


  const startVal = document.getElementById("schedule-start").value;
  const endVal = document.getElementById("schedule-end").value;
  const startParts = startVal ? startVal.split(":") : ["9", "0"];
  const endParts = endVal ? endVal.split(":") : ["17", "0"];

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
    schedule_start_hour: parseInt(startParts[0], 10) || 0,
    schedule_start_minute: parseInt(startParts[1], 10) || 0,
    schedule_end_hour: parseInt(endParts[0], 10) || 0,
    schedule_end_minute: parseInt(endParts[1], 10) || 0,
    schedule_days: scheduleDays,
    profiles: originalConfig.profiles || [],
    active_profile: document.getElementById("profile-select").value || "Default",
    wifi: {
      enabled: document.getElementById("app-mode").value === "WiFi",
      networks: originalConfig.wifi?.networks || [],
    },
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

  // Inject a Recheck button into the banner so the user can trigger a
  // manual recheck, and start an automatic polling loop (5 s × 6 = 30 s).
  const banner = document.getElementById("accessibility-banner");
  if (!banner || banner.classList.contains("hidden")) return;

  // Avoid duplicate recheck buttons if the user clicks Grant multiple times.
  if (!document.getElementById("recheck-btn")) {
    const recheckBtn = document.createElement("button");
    recheckBtn.id = "recheck-btn";
    recheckBtn.className = "link";
    recheckBtn.textContent = "Recheck";
    recheckBtn.addEventListener("click", () => checkAccessibility());
    banner.appendChild(recheckBtn);
  }

  // Automatic poll: check every 5 s for up to 30 s (6 attempts).
  let attempts = 0;
  const MAX_ATTEMPTS = 6;
  const POLL_INTERVAL_MS = 5000;

  const poll = setInterval(async () => {
    attempts++;
    try {
      const granted = await invoke("check_accessibility");
      if (granted) {
        clearInterval(poll);
        // Re-use existing checkAccessibility() to update the banner state.
        await checkAccessibility();
        return;
      }
    } catch (_) {
      // Ignore transient errors — keep polling.
    }
    if (attempts >= MAX_ATTEMPTS) {
      clearInterval(poll);
    }
  }, POLL_INTERVAL_MS);
}

async function saveProfileAs() {
  const raw = prompt("Profile name:");
  if (!raw || !raw.trim()) return;
  const name = raw.trim().replace(/[\/\\:*?"<>|.\x00]/g, "").substring(0, 64);
  if (!name) return;
  try {
    await invoke("save_profile", { name });
    await loadProfiles(name);
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

// ── WiFi functions ──

let currentSsid = null;

async function loadWifiState(cfg) {
  // Check Location Services status
  try {
    const locStatus = await invoke("get_location_status");
    updateLocationBanner(locStatus);
  } catch (_) {}

  // Fetch current SSID
  try {
    currentSsid = await invoke("get_current_wifi");
  } catch (_) {
    currentSsid = null;
  }
  updateWifiDisplay(cfg);
}

function updateLocationBanner(status) {
  let banner = document.getElementById("wifi-location-banner");
  if (status === "authorized") {
    if (banner) banner.style.display = "none";
    return;
  }

  if (!banner) {
    // Create the banner dynamically
    const details = document.getElementById("wifi-details");
    if (!details) return;
    banner = document.createElement("div");
    banner.id = "wifi-location-banner";
    banner.className = "wifi-location-banner";
    details.insertBefore(banner, details.firstChild);
  }

  banner.style.display = "";
  if (status === "not_determined") {
    banner.innerHTML = '<span>📍 Location permission required to detect WiFi networks.</span>' +
      '<button id="wifi-grant-location" class="small">Grant Access</button>';
    const grantBtn = document.getElementById("wifi-grant-location");
    if (grantBtn) {
      grantBtn.addEventListener("click", async () => {
        try {
          await invoke("request_location_permission");
          // Re-check after a short delay (dialog may take time)
          setTimeout(async () => {
            const newStatus = await invoke("get_location_status");
            updateLocationBanner(newStatus);
            if (newStatus === "authorized") {
              currentSsid = await invoke("get_current_wifi");
              const cfg = await invoke("get_config");
              updateWifiDisplay(cfg);
            }
          }, 2000);
        } catch (e) {
          console.error("Failed to request location:", e);
        }
      });
    }
  } else if (status === "denied" || status === "restricted") {
    banner.innerHTML = '<span>⚠️ Location access denied. Enable it in System Settings → Privacy & Security → Location Services.</span>';
  }
}

function updateWifiDisplay(cfg) {
  const ssidEl = document.getElementById("wifi-current-ssid");
  const registerBtn = document.getElementById("wifi-register-btn");
  const networks = cfg?.wifi?.networks || [];

  if (currentSsid) {
    ssidEl.textContent = currentSsid;
    ssidEl.classList.remove("disconnected");
    // Enable register button only if not already registered
    const alreadyRegistered = networks.includes(currentSsid);
    registerBtn.disabled = alreadyRegistered;
  } else {
    ssidEl.setAttribute("data-i18n", "settings-wifi-disconnected");
    ssidEl.textContent = "Not connected";
    ssidEl.classList.add("disconnected");
    registerBtn.disabled = true;
    // Apply translation for "Not connected"
    invoke("get_translation", { key: "settings-wifi-disconnected" })
      .then((text) => { ssidEl.textContent = text; })
      .catch(() => {});
  }

  renderNetworkList(networks);
}

function renderNetworkList(networks) {
  const container = document.getElementById("wifi-network-list");
  container.innerHTML = "";

  if (networks.length === 0) {
    const none = document.createElement("span");
    none.className = "wifi-none";
    none.setAttribute("data-i18n", "settings-wifi-none");
    none.textContent = "No networks registered";
    invoke("get_translation", { key: "settings-wifi-none" })
      .then((text) => { none.textContent = text; })
      .catch(() => {});
    container.appendChild(none);
    return;
  }

  for (const ssid of networks) {
    const item = document.createElement("div");
    item.className = "wifi-network-item";

    const name = document.createElement("span");
    name.className = "wifi-network-name";
    name.textContent = ssid;
    // Highlight if it's the current network
    if (ssid === currentSsid) {
      name.classList.add("current");
    }

    const removeBtn = document.createElement("button");
    removeBtn.className = "wifi-remove-btn";
    removeBtn.innerHTML = '<i class="ph ph-x"></i>';
    removeBtn.title = "Remove";
    removeBtn.addEventListener("click", () => removeNetwork(ssid));

    item.appendChild(name);
    item.appendChild(removeBtn);
    container.appendChild(item);
  }
}

async function registerCurrentNetwork() {
  if (!currentSsid) return;

  // Get current config, add network, save
  try {
    const cfg = await invoke("get_config");
    if (!cfg.wifi.networks.includes(currentSsid)) {
      cfg.wifi.networks.push(currentSsid);
      await invoke("save_config", { newConfig: cfg });
      originalConfig = cfg;
      updateWifiDisplay(cfg);
    }
  } catch (e) {
    console.error("Failed to register network:", e);
  }
}

async function removeNetwork(ssid) {
  try {
    const cfg = await invoke("get_config");
    cfg.wifi.networks = cfg.wifi.networks.filter((n) => n !== ssid);
    await invoke("save_config", { newConfig: cfg });
    originalConfig = cfg;
    updateWifiDisplay(cfg);
  } catch (e) {
    console.error("Failed to remove network:", e);
  }
}

// Hotkey recorder
let isRecording = false;

function setupHotkeyRecorder() {
  const btn = document.getElementById("hotkey-recorder");
  const kbd = document.getElementById("hotkey");
  const hint = document.querySelector(".hotkey-hint");

  function stopRecording() {
    isRecording = false;
    btn.classList.remove("recording");
    hint.textContent = "Click to record";
    hint.setAttribute("data-i18n", "settings-hotkey-hint");
  }

  function buildDisplay(e) {
    const parts = [];
    if (e.metaKey) parts.push("\u2318 Cmd");
    if (e.ctrlKey && !e.metaKey) parts.push("Ctrl");
    if (e.altKey) parts.push("\u2325 Alt");
    if (e.shiftKey) parts.push("\u21e7 Shift");
    return parts;
  }

  btn.addEventListener("click", () => {
    if (isRecording) {
      kbd.textContent = formatHotkeyDisplay(originalConfig.global_hotkey);
      stopRecording();
      return;
    }
    isRecording = true;
    btn.classList.add("recording");
    kbd.textContent = "...";
    hint.setAttribute("data-i18n", "settings-hotkey-recording");
    hint.textContent = "Press keys...";
  });

  // Show modifiers in real-time as user presses them
  document.addEventListener("keydown", async (e) => {
    if (!isRecording) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      kbd.textContent = formatHotkeyDisplay(originalConfig.global_hotkey);
      stopRecording();
      return;
    }

    const displayParts = buildDisplay(e);

    // If it's just a modifier key, show it but don't finalize
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) {
      kbd.textContent = displayParts.length > 0 ? displayParts.join("+") + "+" : "...";
      return;
    }

    // Non-modifier key pressed — need at least one modifier
    if (displayParts.length === 0) return;

    let keyName = e.key.length === 1 ? e.key.toUpperCase() : e.key;
    if (keyName === " ") keyName = "Space";

    // Show final combo
    displayParts.push(keyName);
    kbd.textContent = displayParts.join("+");
    stopRecording();

    // Build backend-compatible string
    const normalized = [];
    if (e.metaKey || e.ctrlKey) normalized.push("CmdOrCtrl");
    if (e.altKey) normalized.push("Alt");
    if (e.shiftKey) normalized.push("Shift");
    normalized.push(keyName.toUpperCase());
    const shortcutStr = normalized.join("+");

    try {
      await invoke("update_global_hotkey", { hotkey: shortcutStr });
      originalConfig.global_hotkey = shortcutStr;
    } catch (err) {
      console.error("Failed to update hotkey:", err);
      kbd.textContent = formatHotkeyDisplay(originalConfig.global_hotkey);
    }
  });

  // Handle modifier key release to update display
  document.addEventListener("keyup", (e) => {
    if (!isRecording) return;
    e.preventDefault();

    const displayParts = buildDisplay(e);
    kbd.textContent = displayParts.length > 0 ? displayParts.join("+") + "+" : "...";
  });
}

window.addEventListener("DOMContentLoaded", () => {
  loadConfig();
  setupHotkeyRecorder();

  document.getElementById("interval").addEventListener("input", (e) => {
    document.getElementById("interval-value").textContent = e.target.value + "s";
  });

  document.getElementById("jiggle-mode").addEventListener("change", () => {
    checkAccessibility();
    autoSave();
  });
  document.getElementById("interval").addEventListener("change", autoSave);
  document.getElementById("app-mode").addEventListener("change", async () => {
    const mode = document.getElementById("app-mode").value;
    const wifiSection = document.getElementById("wifi-section");

    if (mode === "WiFi") {
      wifiSection.style.display = "";
      // Proactively request location if status is not_determined — no banner click needed.
      try {
        const locStatus = await invoke("get_location_status");
        if (locStatus === "not_determined") {
          await invoke("request_location_permission");
        }
        updateLocationBanner(locStatus);
      } catch (_) {}
      // Load current SSID and network list.
      try {
        const cfg = await invoke("get_config");
        await loadWifiState(cfg);
      } catch (_) {}
    } else {
      wifiSection.style.display = "none";
    }

    updateModeDescription();
    autoSave();
  });
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

  // WiFi section
  document.getElementById("wifi-register-btn").addEventListener("click", registerCurrentNetwork);

  // Listen for real-time WiFi state changes from the backend
  listen("wifi-state-changed", (event) => {
    const payload = event.payload;
    currentSsid = payload.ssid || null;
    // Refresh the display with current config
    invoke("get_config").then((cfg) => {
      originalConfig = cfg;
      updateWifiDisplay(cfg);
    }).catch(() => {});
    // Also refresh location status
    invoke("get_location_status").then((status) => {
      updateLocationBanner(status);
    }).catch(() => {});
  });
});
