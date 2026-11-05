/**
 * @spec-handoff
 * @interface loadConfig(): Promise<void>; autoSave(): Promise<void>
 * @behavior
 *   - Places the complete App Mode control before Jiggle Mode and Interval.
 *   - Keeps Manual, AlwaysOn, and WiFi as the only selectable App Modes.
 *   - Preserves midnight schedule values through loading and autosave.
 *   - Associates the schedule helper and localized descriptions with their controls.
 * @edge-cases
 *   - Legacy Scheduled displays as Manual and autosaves as Manual.
 *   - Empty days and disabled schedules retain stored midnight values.
 * @see ../src/settings.js
 */

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";

const root = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const settingsHtml = read("src/settings.html");
const settingsJs = read("src/settings.js");
const settingsCss = read("src/settings.css");
const locales = readdirSync(new URL("src-tauri/locales/", root)).filter((name) => name.endsWith(".ftl"));

const fieldStart = (source, id) => source.lastIndexOf('<label class="field"', source.indexOf(`id="${id}"`));
const divFieldStart = (source, id) => source.lastIndexOf('<div class="field"', source.indexOf(`id="${id}"`));
const optionValues = (source) => [...source.matchAll(/<option value="([^"]+)"/g)].map((match) => match[1]);
const appModeSelect = (source) => source.match(/<select id="app-mode">([\s\S]*?)<\/select>/)?.[1] ?? "";
const appModeBlock = (source) => {
  const start = fieldStart(source, "app-mode");
  const end = source.indexOf("</label>", start);
  return start >= 0 && end >= 0 ? source.slice(start, end + "</label>".length) : "";
};

const assertSettingsOrder = (html, css) => {
  const app = fieldStart(html, "app-mode");
  const jiggle = divFieldStart(html, "jiggle-mode");
  const interval = fieldStart(html, "interval");
  const launch = fieldStart(html, "autostart");
  assert.ok(app >= 0 && jiggle >= 0 && interval >= 0 && launch >= 0, "each control needs an enclosing field");
  assert.ok(app < jiggle, "App Mode block must precede the Jiggle Mode block");
  assert.ok(jiggle < interval, "Jiggle Mode block must precede Interval");
  assert.ok(jiggle < launch, "Launch at Login must remain after Jiggle Mode");
  assert.match(appModeBlock(html), /id="app-mode-desc"/, "App Mode description must stay in its label");
  assert.equal(/\btabindex\s*=\s*["']?[1-9]/i.test(html), false, "positive tabindex is forbidden");
  assert.equal(/\border\s*:/i.test(css), false, "CSS must not impose settings control order");
  const focusable = /<(?:select|input|button)\b[^>]*\bid="([^"]+)"/g;
  assert.equal(focusable.exec(html)?.[1], "app-mode", "App Mode must be first focusable control");
};

const localeKeys = (source) => new Set([...source.matchAll(/^([\w-]+)\s*=/gm)].map((match) => match[1]));

class Element {
  constructor(id, value = "") {
    this.id = id;
    this.value = value;
    this.checked = false;
    this.textContent = "";
    this.style = {};
    this.listeners = new Map();
    this.classList = { add() {}, remove() {}, toggle() {}, contains: () => false };
  }

  addEventListener(type, listener) { this.listeners.set(type, listener); }
  dispatch(type) { return this.listeners.get(type)?.({ target: this }); }
  setAttribute() {}
  getAttribute() { return null; }
  appendChild() {}
}

const runSettings = async (config) => {
  const ids = ["jiggle-mode", "interval", "interval-value", "app-mode", "app-mode-desc", "autostart",
    "language", "hotkey", "schedule-enabled", "schedule-start", "schedule-end", "profile-select",
    "wifi-section", "accessibility-banner", "mode-info-btn", "grant-btn", "profile-save-btn",
    "profile-delete-btn", "wifi-register-btn", "hotkey-recorder"];
  const elements = new Map(ids.map((id) => [id, new Element(id)]));
  const days = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"].map((value) => new Element("", value));
  const documentListeners = new Map();
  const saves = [];
  const invoke = async (command, args) => {
    if (command === "get_config") return config;
    if (command === "list_profiles") return [];
    if (command === "check_accessibility") return true;
    if (command === "save_config") saves.push(args.newConfig);
    if (command === "get_platform_os") return "linux";
    if (command === "get_translation") return "translated";
    return undefined;
  };
  const document = {
    getElementById: (id) => elements.get(id),
    querySelector: () => null,
    querySelectorAll: (selector) => selector.includes("day-checkboxes") ? days : [],
    addEventListener: (type, listener) => documentListeners.set(type, listener),
    createElement: () => new Element(),
  };
  const window = {
    __TAURI__: { core: { invoke }, event: { listen: async () => {} } },
    addEventListener: (type, listener) => { if (type === "DOMContentLoaded") documentListeners.set(type, listener); },
  };
  const context = vm.createContext({ window, document, navigator: { platform: "Linux" }, console, setInterval,
    clearInterval, setTimeout, clearTimeout, prompt: () => null });
  vm.runInContext(settingsJs, context, { filename: "settings.js" });
  documentListeners.get("DOMContentLoaded")();
  for (let tick = 0; tick < 12; tick += 1) await Promise.resolve();
  return { elements, days, saves };
};

test("places complete App Mode first without CSS or tabindex ordering", () => {
  assertSettingsOrder(settingsHtml, settingsCss);
});

test("rejects App Mode order contract mutants", () => {
  const app = appModeBlock(settingsHtml);
  const green = settingsHtml.replace(app, "").replace('<section class="group">', `<section class="group">${app}`);
  assertSettingsOrder(green, settingsCss);
  const reordered = green.replace(app, "").replace('</section>', `\n${app}</section>`);
  assert.throws(() => assertSettingsOrder(reordered, settingsCss), /App Mode block/);
  assert.throws(() => assertSettingsOrder(green.replace('id="app-mode-desc"', 'id="detached-desc"'), settingsCss), /description/);
  assert.throws(() => assertSettingsOrder(green.replace('id="app-mode"', 'id="app-mode" tabindex="1"'), settingsCss), /tabindex/);
  const launch = green.match(/\s*<label class="field row">[\s\S]*?id="autostart"[\s\S]*?<\/label>/)?.[0];
  const launchFirst = green.replace(launch, "").replace('<section class="group">', `<section class="group">${launch}`);
  assert.throws(() => assertSettingsOrder(launchFirst, settingsCss));
});

test("keeps exactly the three supported App Mode selector values", () => {
  assert.deepEqual(optionValues(appModeSelect(settingsHtml)), ["Manual", "AlwaysOn", "WiFi"]);
  assert.doesNotMatch(settingsJs, /app-mode[\s\S]{0,400}Scheduled/, "settings.js must not add Scheduled to App Mode");
  assert.throws(() => assert.deepEqual(optionValues(`${appModeSelect(settingsHtml)}<option value="Scheduled">Scheduled</option>`), ["Manual", "AlwaysOn", "WiFi"]));
});

test("links the schedule helper and mode descriptions to every locale", () => {
  const helperId = settingsHtml.match(/id="schedule-enabled"[^>]*aria-describedby="([^"]+)"/)?.[1];
  assert.ok(helperId, "schedule checkbox must describe its helper");
  assert.match(settingsHtml, new RegExp(`id="${helperId}"[^>]*data-i18n="schedule-enabled-hint"`));
  const required = new Set(["schedule-enabled-hint", "app-mode-desc-manual", "app-mode-desc-always-on"]);
  for (const locale of locales) for (const key of required) {
    assert.ok(localeKeys(read(`src-tauri/locales/${locale}`)).has(key), `${locale} is missing ${key}`);
  }
});

test("round-trips midnight schedules through the actual Settings load and autosave paths", async () => {
  const config = {
    jiggle_mode: "PowerOnly", interval_secs: 30, mode: "Scheduled", autostart: false, language: "en",
    global_hotkey: "CmdOrCtrl+Shift+J", schedule_enabled: false, schedule_start_hour: 0,
    schedule_start_minute: 30, schedule_end_hour: 0, schedule_end_minute: 0, schedule_days: [],
    profiles: [], active_profile: "Default", wifi: { networks: [] },
  };
  const { elements, days, saves } = await runSettings(config);
  assert.equal(elements.get("app-mode").value, "Manual", "legacy Scheduled displays as Manual");
  assert.equal(elements.get("schedule-start").value, "00:30");
  assert.equal(elements.get("schedule-end").value, "00:00");
  assert.deepEqual(days.map((day) => day.checked), [false, false, false, false, false, false, false]);
  await elements.get("interval").dispatch("change");
  assert.equal(saves.length, 1, "an unrelated autosave must persist loaded schedule fields");
  assert.equal(saves[0].schedule_start_hour, 0);
  assert.equal(saves[0].schedule_start_minute, 30);
  assert.equal(saves[0].schedule_end_hour, 0);
  assert.equal(saves[0].schedule_end_minute, 0);
  assert.equal(saves[0].mode, "Manual", "legacy Scheduled autosaves as Manual");
});
