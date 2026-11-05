/**
 * @spec-handoff
 * @interface loadConfig(): Promise<void>; autoSave(): Promise<void>
 * @behavior
 *   - Places Launch at Login in the first group and App Mode in the second group.
 *   - Keeps Manual, AlwaysOn, and WiFi as the only selectable App Modes.
 *   - Preserves midnight schedule values through loading and autosave.
 *   - Associates the schedule helper and localized descriptions with their controls.
 *   - Shows the localized Schedule warning only while WiFi is selected.
 *   - Renders network removal names from translated Fluent arguments without frontend substitution.
 * @edge-cases
 *   - Legacy Scheduled displays as Manual and autosaves as Manual.
 *   - Empty days and disabled schedules retain stored midnight values.
 * @see ../src/settings.js
 */

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";

import { stripHtmlComments } from "./helpers/strip-html-comments.mjs";

const root = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const settingsHtml = read("src/settings.html");
const settingsJs = read("src/settings.js");
const settingsCss = read("src/settings.css");
const locales = readdirSync(new URL("src-tauri/locales/", root)).filter((name) => name.endsWith(".ftl"));

const uncommented = stripHtmlComments;
const attributes = (tag) => Object.fromEntries([...tag.matchAll(/([:\w-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g)]
  .map(([, name, doubleQuoted, singleQuoted, bare]) => [name, doubleQuoted ?? singleQuoted ?? bare ?? ""]));
const tagWithId = (source, id) => [...source.matchAll(/<\w+\b[^>]*>/g)]
  .map((match) => ({ tag: match[0], index: match.index, attributes: attributes(match[0]) }))
  .find((entry) => entry.attributes.id === id);
const enclosingFieldStart = (source, id) => {
  const control = tagWithId(source, id);
  if (!control) return -1;
  return [...source.slice(0, control.index).matchAll(/<(?:label|div)\b[^>]*>/g)]
    .filter((match) => attributes(match[0]).class?.split(/\s+/).includes("field"))
    .at(-1)?.index ?? -1;
};
const optionValues = (source) => [...source.matchAll(/<option\b[^>]*>/g)]
  .map((match) => attributes(match[0]).value)
  .filter((value) => value !== undefined);
const appModeSelect = (source) => {
  const select = tagWithId(source, "app-mode");
  return select ? source.slice(select.index, source.indexOf("</select>", select.index) + "</select>".length) : "";
};
const appModeBlock = (source) => {
  const start = enclosingFieldStart(source, "app-mode");
  const end = source.indexOf("</label>", start);
  return start >= 0 && end >= 0 ? source.slice(start, end + "</label>".length) : "";
};

const settingsGroups = (source) => [...source.matchAll(/<section\b[^>]*class="group"[^>]*>/g)]
  .map((match) => {
    const end = source.indexOf("</section>", match.index);
    return end < 0 ? "" : source.slice(match.index, end + "</section>".length);
  });
const swapFirstTwoGroups = (source) => {
  const groups = settingsGroups(source);
  const first = source.indexOf(groups[0]);
  const second = source.indexOf(groups[1]);
  return source.slice(0, first) + groups[1] + source.slice(first + groups[0].length, second)
    + groups[0] + source.slice(second + groups[1].length);
};

const assertSettingsOrder = (html, css) => {
  const source = uncommented(html);
  const groups = settingsGroups(source);
  const app = enclosingFieldStart(source, "app-mode");
  const jiggle = enclosingFieldStart(source, "jiggle-mode");
  const interval = enclosingFieldStart(source, "interval");
  const launch = enclosingFieldStart(source, "autostart");
  assert.ok(app >= 0 && jiggle >= 0 && interval >= 0 && launch >= 0, "each control needs an enclosing field");
  assert.ok(groups[0]?.includes('id="autostart"'), "Launch at Login must occupy the first group");
  assert.ok(groups[1]?.includes('id="app-mode"'), "App Mode must occupy the second group");
  assert.ok(launch < app, "Launch at Login must precede App Mode");
  assert.ok(app < jiggle, "App Mode block must precede the Jiggle Mode block");
  assert.ok(jiggle < interval, "Jiggle Mode block must precede Interval");
  assert.ok(tagWithId(appModeBlock(source), "app-mode-desc"), "App Mode description must stay in its label");
  assert.equal(/\btabindex\s*=\s*["']?[1-9]/i.test(source), false, "positive tabindex is forbidden");
  assert.equal(/\border\s*:/i.test(css), false, "CSS must not impose settings control order");
  const focusable = [...source.matchAll(/<(?:select|input|button)\b[^>]*>/g)]
    .map((match) => attributes(match[0]).id)
    .filter(Boolean);
  assert.deepEqual(focusable.slice(0, 5), ["autostart", "app-mode", "mode-info-btn", "jiggle-mode", "interval"],
    "Launch at Login, App Mode, Info, Jiggle Mode, and Interval must be the first focusable controls");
};

const localeKeys = (source) => new Set([...source.matchAll(/^([\w-]+)\s*=/gm)].map((match) => match[1]));
const localeValue = (source, key) => source.match(new RegExp(`^${key}\\s*=\\s*(.+)$`, "m"))?.[1]?.trim();
const stripCssComments = (source) => source.replace(/\/\*[\s\S]*?\*\//g, "");
const cssBlock = (source, selector) => stripCssComments(source)
  .match(new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\{([^}]*)\\}`, "s"))?.[1] ?? "";
const assertScheduleAlertHiddenState = (html, js, css) => {
  const schedule = settingsGroups(uncommented(html)).find((group) => group.includes('id="schedule-section"'));
  const alert = tagWithId(schedule ?? "", "schedule-wifi-alert");
  assert.ok("hidden" in (alert?.attributes ?? {}), "Schedule alert must start with the hidden attribute");
  const update = js.match(/function updateScheduleWifiAlert\([^)]*\)\s*\{([\s\S]*?)\n\}/)?.[1] ?? "";
  assert.match(update, /\.hidden\s*=\s*mode\s*!==\s*["']WiFi["']/,
    "Schedule alert updates must use the HTML hidden state");
  assert.match(cssBlock(css, ".schedule-alert"), /display\s*:\s*flex\s*;/,
    "Schedule alert base layout must remain flex");
  assert.match(cssBlock(css, ".schedule-alert[hidden]"), /display\s*:\s*none\s*!important\s*;/,
    "Schedule alert hidden state must override its flex layout");
};

// Mutant: restore the descendant selector and let nested hints inherit group padding.
test("keeps schedule helper padding scoped to a direct group child", () => {
  assert.match(settingsCss, /\.group\s*>\s*\.field-hint\s*\{[^}]*padding:\s*0\s+16px\s*;/s);
  assert.doesNotMatch(settingsCss, /\.group\s+\.field-hint\s*\{/);
});

class Element {
  constructor(id, value = "") {
    this.id = id;
    this.value = value;
    this.checked = false;
    this.hidden = false;
    this.textContent = "";
    this.style = {};
    this.listeners = new Map();
    this.attributes = new Map();
    this.tagName = "DIV";
    this.classList = { add() {}, remove() {}, toggle() {}, contains: () => false };
  }

  addEventListener(type, listener) { this.listeners.set(type, listener); }
  dispatch(type) { return this.listeners.get(type)?.({ target: this }); }
  setAttribute(name, value) { this.attributes.set(name, value); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  appendChild() {}
}

const waitFor = async (predicate, message) => {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.fail(message);
};

const runSettings = async (config) => {
  const ids = ["jiggle-mode", "interval", "interval-value", "app-mode", "app-mode-desc", "autostart",
    "language", "hotkey", "schedule-enabled", "schedule-start", "schedule-end", "profile-select",
    "wifi-section", "accessibility-banner", "mode-info-btn", "grant-btn", "profile-save-btn",
    "profile-delete-btn", "wifi-register-btn", "wifi-current-ssid", "wifi-network-list", "hotkey-recorder",
    "schedule-wifi-alert"];
  const elements = new Map(ids.map((id) => [id, new Element(id)]));
  const alert = elements.get("schedule-wifi-alert");
  alert.hidden = true;
  alert.setAttribute("data-i18n", "schedule-wifi-alert");
  const days = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"].map((value) => new Element("", value));
  const documentListeners = new Map();
  const saves = [];
  let activeLanguage = config.language;
  const invoke = async (command, args) => {
    if (command === "get_config") return config;
    if (command === "list_profiles") return [];
    if (command === "check_accessibility") return true;
    if (command === "save_config") saves.push(args.newConfig);
    if (command === "get_platform_os") return "linux";
    if (command === "get_translation") return `${activeLanguage}:${args.key}`;
    if (command === "set_language") activeLanguage = args.language;
    return undefined;
  };
  const document = {
    getElementById: (id) => elements.get(id),
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector === ".day-checkboxes input:checked") return days.filter((day) => day.checked);
      if (selector.includes("day-checkboxes")) return days;
      if (selector === "[data-i18n]") return [...elements.values()].filter((el) => el.getAttribute("data-i18n"));
      if (selector === "[data-i18n-aria-label]") return [];
      return [];
    },
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
  await waitFor(() => elements.get("jiggle-mode").value === config.jiggle_mode, "Settings did not load config");
  return { elements, days, saves };
};

test("places Launch at Login first without CSS or tabindex ordering", () => {
  assertSettingsOrder(settingsHtml, settingsCss);
});

test("rejects Launch at Login order contract mutants", () => {
  const green = settingsHtml;
  assertSettingsOrder(green, settingsCss);
  assert.throws(() => assertSettingsOrder(swapFirstTwoGroups(green), settingsCss), /Launch at Login must occupy the first group/);
  assert.throws(() => assertSettingsOrder(green.replace('id="app-mode-desc"', 'id="detached-desc"'), settingsCss), /description/);
  assert.throws(() => assertSettingsOrder(green.replace('id="app-mode"', 'id="app-mode" tabindex="1"'), settingsCss), /tabindex/);
  const app = appModeBlock(green);
  assert.throws(() => assertSettingsOrder(green.replace(app, `<!--${app}-->`), settingsCss), /enclosing field/);
});

test("keeps the WiFi-only Schedule alert localized, hidden by default, and visually inset", () => {
  const schedule = settingsGroups(uncommented(settingsHtml)).find((group) => group.includes('id="schedule-section"'));
  const alert = tagWithId(schedule ?? "", "schedule-wifi-alert");
  assert.ok(alert, "Schedule needs a WiFi-only alert element");
  assert.ok("hidden" in alert.attributes, "Schedule alert must start hidden");
  assert.ok(["status", "alert"].includes(alert.attributes.role), "Schedule alert needs an accessible status role");
  const alertMarkup = (schedule ?? "").slice(alert.index, (schedule ?? "").indexOf("</div>", alert.index) + "</div>".length);
  assert.match(alertMarkup, /data-i18n="schedule-wifi-alert"/);
  assert.match(alertMarkup, /<svg\b[^>]*aria-hidden="true"[^>]*focusable="false"/);
  assert.doesNotMatch(alertMarkup, /[\u{1F300}-\u{1FAFF}]/u, "Schedule alert must not use emoji");
  assert.match(settingsCss, /\.schedule-alert\s*\{[^}]*padding\s*:/s, "Schedule alert needs its own inset rule");
});

test("requires the Schedule alert hidden state to override its flex layout", () => {
  assertScheduleAlertHiddenState(settingsHtml, settingsJs, settingsCss);
  const withoutHiddenRule = settingsCss.replace(/\.schedule-alert\[hidden\]\s*\{[^}]*\}\s*/s, "");
  assert.throws(() => assertScheduleAlertHiddenState(settingsHtml, settingsJs, withoutHiddenRule),
    /hidden state must override/);
});

test("shows the Schedule alert only for WiFi on load and live mode changes", async () => {
  const wifi = await runSettings({ ...baseConfig(), mode: "WiFi" });
  assert.equal(wifi.elements.get("schedule-wifi-alert").hidden, false, "WiFi shows the Schedule alert on load");

  for (const mode of ["Manual", "AlwaysOn", "Scheduled"]) {
    const state = await runSettings({ ...baseConfig(), mode });
    assert.equal(state.elements.get("schedule-wifi-alert").hidden, true, `${mode} keeps the Schedule alert hidden`);
  }

  const live = await runSettings(baseConfig());
  const select = live.elements.get("app-mode");
  select.value = "WiFi";
  const change = select.dispatch("change");
  assert.equal(live.elements.get("schedule-wifi-alert").hidden, false, "WiFi change updates the alert before saving");
  await change;
  select.value = "Manual";
  await select.dispatch("change");
  assert.equal(live.elements.get("schedule-wifi-alert").hidden, true, "leaving WiFi hides the alert");
});

test("keeps the WiFi Schedule alert visible through re-localization", async () => {
  const state = await runSettings({ ...baseConfig(), mode: "WiFi" });
  const alert = state.elements.get("schedule-wifi-alert");
  assert.equal(alert.hidden, false);
  state.elements.get("language").value = "es";
  await state.elements.get("language").dispatch("change");
  assert.equal(alert.hidden, false, "re-localizing must not hide the WiFi alert");
  assert.equal(alert.textContent, "es:schedule-wifi-alert");
});

test("keeps exactly the three supported App Mode selector values", async () => {
  assert.deepEqual(optionValues(appModeSelect(settingsHtml)), ["Manual", "AlwaysOn", "WiFi"]);
  assert.throws(() => assert.deepEqual(optionValues(`${appModeSelect(settingsHtml)}<option value="Scheduled">Scheduled</option>`), ["Manual", "AlwaysOn", "WiFi"]));
  const { elements, saves } = await runSettings({ ...baseConfig(), mode: "Scheduled" });
  assert.equal(elements.get("app-mode").value, "Manual", "legacy Scheduled displays as Manual");
  await elements.get("interval").dispatch("change");
  assert.equal(saves[0].mode, "Manual", "legacy Scheduled autosaves as Manual");
});

test("links the schedule helper and requires it in exactly eight locales", () => {
  assert.equal(locales.length, 8, "the shipped locale set must contain exactly eight files");
  const helperId = tagWithId(settingsHtml, "schedule-enabled")?.attributes["aria-describedby"];
  assert.ok(helperId, "schedule checkbox must describe its helper");
  assert.equal(tagWithId(settingsHtml, helperId)?.attributes["data-i18n"], "schedule-enabled-hint");
  const required = new Set(["schedule-enabled-hint"]);
  for (const locale of locales) for (const key of required) {
    assert.ok(localeKeys(read(`src-tauri/locales/${locale}`)).has(key), `${locale} is missing ${key}`);
  }
});

test("separates the Manual and Always On helper from the WiFi warning in every locale", () => {
  const hint = tagWithId(settingsHtml, "schedule-enabled-hint");
  assert.doesNotMatch(hint?.tag ?? "", /wifi/i, "Schedule helper markup must not mention WiFi");
  const englishHint = localeValue(read("src-tauri/locales/en.ftl"), "schedule-enabled-hint");
  assert.equal(englishHint, "Automatically starts and stops the engine with the schedule.");
  for (const locale of locales) {
    const source = read(`src-tauri/locales/${locale}`);
    const alert = localeValue(source, "schedule-wifi-alert");
    const localizedHint = localeValue(source, "schedule-enabled-hint");
    assert.ok(alert, `${locale} needs a non-empty WiFi Schedule alert`);
    assert.notEqual(alert, localizedHint, `${locale} must not repeat the Schedule helper as its alert`);
  }
});

test("keeps Manual and AlwaysOn descriptions aligned with the Settings contract", () => {
  const block = settingsJs.match(/const MODE_DESCRIPTIONS\s*=\s*\{([\s\S]*?)\n\};/)?.[1] ?? "";
  const description = (mode) => block.match(new RegExp(`(?:${mode}|["']${mode}["'])\\s*:\\s*["']([^"']*)["']`))?.[1];
  assert.equal(description("Manual"), "Use the tray or hotkey to start and stop. An enabled schedule also controls automatic starts and stops.");
  assert.equal(description("AlwaysOn"), "Starts automatically. An enabled schedule limits operation to its windows.");
});

const baseConfig = () => ({
  jiggle_mode: "PowerOnly", interval_secs: 30, mode: "Manual", autostart: false, language: "en",
  global_hotkey: "CmdOrCtrl+Shift+J", schedule_enabled: false, schedule_start_hour: 0,
  schedule_start_minute: 30, schedule_end_hour: 0, schedule_end_minute: 0, schedule_days: [],
  profiles: [], active_profile: "Default", wifi: { networks: [] },
});

test("round-trips midnight schedules through the actual Settings load and autosave paths", async () => {
  const config = { ...baseConfig(), mode: "Scheduled" };
  const { elements, days, saves } = await runSettings(config);
  assert.equal(elements.get("app-mode").value, "Manual", "legacy Scheduled displays as Manual");
  assert.equal(elements.get("schedule-enabled").checked, false, "disabled schedule must load unchecked");
  assert.equal(elements.get("schedule-start").value, "00:30");
  assert.equal(elements.get("schedule-end").value, "00:00");
  assert.deepEqual(days.map((day) => day.checked), [false, false, false, false, false, false, false]);
  await elements.get("interval").dispatch("change");
  assert.equal(saves.length, 1, "an unrelated autosave must persist loaded schedule fields");
  assert.equal(saves[0].schedule_start_hour, 0);
  assert.equal(saves[0].schedule_start_minute, 30);
  assert.equal(saves[0].schedule_end_hour, 0);
  assert.equal(saves[0].schedule_end_minute, 0);
  assert.deepEqual(Array.from(saves[0].schedule_days), [], "empty day selection must remain empty");
  assert.equal(saves[0].schedule_enabled, false, "disabled schedule must remain disabled");
  assert.equal(saves[0].mode, "Manual", "legacy Scheduled autosaves as Manual");
});

test("preserves checked schedule days and disabled stored times during autosave", async () => {
  const { elements, days, saves } = await runSettings({ ...baseConfig(), schedule_days: ["mon", "fri"] });
  assert.deepEqual(days.map((day) => day.checked), [true, false, false, false, true, false, false]);
  await elements.get("interval").dispatch("change");
  assert.deepEqual(Array.from(saves[0].schedule_days), ["mon", "fri"]);
  assert.equal(saves[0].schedule_enabled, false);
  assert.equal(saves[0].schedule_start_hour, 0);
  assert.equal(saves[0].schedule_end_hour, 0);
});

test("loads midnight start and missing schedule-hour defaults", async () => {
  const midnight = await runSettings({ ...baseConfig(), schedule_start_minute: 0, schedule_end_hour: 17 });
  assert.equal(midnight.elements.get("schedule-start").value, "00:00");
  const missing = await runSettings(Object.fromEntries(Object.entries(baseConfig()).filter(([key]) => ![
    "schedule_start_hour", "schedule_start_minute", "schedule_end_hour", "schedule_end_minute",
  ].includes(key))));
  assert.equal(missing.elements.get("schedule-start").value, "09:00");
  assert.equal(missing.elements.get("schedule-end").value, "17:00");
});

test("renders literal translated network names without frontend placeholder substitution", async () => {
  class NetworkElement {
    constructor(leaf = false) {
      this.attributes = new Map();
      this.children = [];
      this.classList = { add() {} };
      this.textContent = "";
      this.firstElementChild = leaf ? null : new NetworkElement(true);
    }

    setAttribute(name, value) { this.attributes.set(name, value); }
    getAttribute(name) { return this.attributes.get(name) ?? null; }
    appendChild(child) { this.children.push(child); }
    addEventListener() {}
  }

  let language = "en";
  const list = new NetworkElement();
  const buttons = [];
  const document = {
    createElement: () => new NetworkElement(),
    getElementById: (id) => id === "wifi-network-list" ? list : new NetworkElement(),
    querySelectorAll: (selector) => selector === "[data-i18n-aria-label]" ? buttons : [],
  };
  const invoke = async (command, args) => {
    if (command !== "get_translation") return undefined;
    const template = language === "en" ? "Remove network {$network}" : "Quitar red {$network}";
    return args.network === undefined ? template : template.replace("{$network}", () => args.network);
  };
  const context = vm.createContext({
    window: {
      __TAURI__: { core: { invoke }, event: { listen: async () => {} } },
      addEventListener() {},
    },
    document,
    navigator: { platform: "Linux" },
    console,
  });
  vm.runInContext(settingsJs, context, { filename: "settings.js" });

  const networks = ["$&", "$$", "$1", "$`", "$'", "plain", "<b>x</b>"];
  context.renderNetworkList(networks);
  await waitFor(() => list.children.length === networks.length, "network list did not render");
  for (const item of list.children) buttons.push(item.children[1]);
  await waitFor(() => buttons.every((button) => button.getAttribute("aria-label")), "names did not translate");
  for (const [index, network] of networks.entries()) {
    assert.equal(buttons[index].getAttribute("aria-label"), `Remove network ${network}`);
    assert.equal(buttons[index].getAttribute("aria-label").split(network).length - 1, 1);
  }

  language = "es";
  await context.applyTranslations();
  for (const [index, network] of networks.entries()) {
    assert.equal(buttons[index].getAttribute("aria-label"), `Quitar red ${network}`);
  }

  const dynamicNames = (settingsJs
    .match(/(?:async )?function (?:applyTranslations|renderNetworkList)[\s\S]*?(?=\nasync function|\nfunction registerCurrentNetwork)/g)
    ?.join("\n") ?? "")
    .replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, "");
  assert.doesNotMatch(dynamicNames, /\.replace(?:All)?\(/,
    "dynamic network labels must rely on Fluent arguments, not frontend substitution");
});
