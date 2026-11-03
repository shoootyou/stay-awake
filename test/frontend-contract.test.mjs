/**
 * @spec-handoff
 * @interface frontendContracts(): void
 * @behavior
 *   - Pins the frontend's hooks, translations, IPC commands, pages, resources, fonts, and icons.
 *   - Keeps the local CSP suitable for shipped scripts, styles, fonts, and images.
 *   - Allows Phosphor to be trimmed to the base rule, font face, and used icon rules.
 * @edge-cases
 *   - Every shipped locale has the English key set.
 *   - Icon names must be literal and cannot be assembled dynamically.
 * @see ../src-tauri/src/lib.rs
 */

import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const read = (path) => readFileSync(join(root, path), "utf8");
const htmlPaths = ["src/index.html", "src/settings.html", "src/about.html"];
const jsPaths = readdirSync(join(root, "src"))
  .filter((name) => name.endsWith(".js"))
  .map((name) => `src/${name}`);
const cssPaths = readdirSync(join(root, "src"), { recursive: true })
  .filter((path) => path.endsWith(".css"))
  .map((path) => `src/${path}`);
const html = Object.fromEntries(htmlPaths.map((path) => [path, read(path)]));
const js = Object.fromEntries(jsPaths.map((path) => [path, read(path)]));
const css = Object.fromEntries(cssPaths.map((path) => [path, read(path)]));
const settingsHtml = html["src/settings.html"];
const settingsJs = js["src/settings.js"];
const phosphorCss = css["src/fonts/phosphor.css"];

const expectedSelectorIds = [
  "accessibility-banner", "app-mode", "app-mode-desc", "autostart", "grant-btn", "hotkey",
  "hotkey-recorder", "interval", "interval-value", "jiggle-mode", "language", "mode-info-btn",
  "mode-info-panel", "profile-delete-btn", "profile-save-btn", "profile-select", "recheck-btn",
  "schedule-enabled", "schedule-end", "schedule-start", "wifi-current-ssid", "wifi-details",
  "wifi-grant-location", "wifi-location-banner", "wifi-network-list", "wifi-nm-banner",
  "wifi-register-btn", "wifi-section",
];
const expectedTranslationKeys = [
  "mode-desc-circle", "mode-desc-power", "mode-desc-subtle", "mode-desc-zen", "profile-default",
  "profile-delete", "profile-label", "profile-save-as", "schedule-days", "schedule-enabled",
  "schedule-end", "schedule-fri", "schedule-label", "schedule-mon", "schedule-sat", "schedule-start",
  "schedule-sun", "schedule-thu", "schedule-tue", "schedule-wed", "settings-accessibility-banner",
  "settings-app-mode-label", "settings-autostart-label", "settings-grant-btn", "settings-hotkey-hint",
  "settings-hotkey-label", "settings-hotkey-recording", "settings-interval-label", "settings-language-label",
  "settings-mode-label", "settings-title", "settings-wifi-current", "settings-wifi-disconnected",
  "settings-wifi-hint", "settings-wifi-nm-required", "settings-wifi-none", "settings-wifi-register",
  "settings-wifi-registered", "settings-wifi-title", "settings-xdotool-banner", "settings-xdotool-install-btn",
  "tray-mode-circle", "tray-mode-power", "tray-mode-subtle", "tray-mode-zen",
];
const expectedCommands = [
  "check_accessibility", "delete_profile", "get_app_version", "get_config", "get_current_wifi",
  "get_location_status", "get_networkmanager_status", "get_platform_os", "get_translation", "list_profiles",
  "load_profile", "request_accessibility", "request_location_permission", "save_config", "save_profile",
  "set_language", "update_global_hotkey",
];
const expectedResources = ["about.css", "app-icon.png", "fonts/phosphor.css", "settings.css"];
const expectedIcons = [
  "ph-app-window", "ph-calendar-blank", "ph-cursor-click", "ph-gear", "ph-info", "ph-keyboard",
  "ph-power", "ph-timer", "ph-translate", "ph-user-circle", "ph-wifi-high", "ph-x",
];
const expectedCsp = "default-src 'self'; style-src 'self'; font-src 'self'; script-src 'self'; "
  + "img-src 'self' asset: https://asset.localhost; connect-src ipc: http://ipc.localhost "
  + "https://github.com https://api.github.com";

const sorted = (values) => [...values].sort();
const attributeValues = (source, attribute) => [...source.matchAll(
  new RegExp(`\\b${attribute}=["']([^"']+)["']`, "g")
)].map((match) => match[1]);
const ids = (source) => new Set(attributeValues(source, "id"));
const fluentKeys = (source) => new Set([...source.matchAll(/^([\w-]+)\s*=/gm)].map((match) => match[1]));
const commandNames = (source) => new Set([...source.matchAll(/invoke\(\s*["']([\w-]+)["']/g)]
  .map((match) => match[1]));
const registeredCommands = (source) => {
  const handler = source.match(/generate_handler!\s*\[([\s\S]*?)\]/)?.[1] ?? "";
  return new Set([...handler.matchAll(/\b([a-z][a-z0-9_]*)\b/g)].map((match) => match[1]));
};
const iconClasses = (source) => new Set([...source.matchAll(/\b(ph-[\w-]+)\b/g)].map((match) => match[1]));
const dynamicIds = (source) => new Set([...source.matchAll(/(?:\.id\s*=|setAttribute\(\s*["']id["']\s*,)\s*["']([\w-]+)/g)]
  .map((match) => match[1]));
const markupIds = (source) => new Set([...source.matchAll(/id=["']([\w-]+)/g)].map((match) => match[1]));
const dynamicClasses = (source) => new Set([...source.matchAll(/\.className\s*=\s*["']([^"']+)/g)]
  .flatMap((match) => match[1].split(/\s+/)));
const resourcePaths = (source) => [...source.matchAll(/(?:href|src)=["']([^"']+)["']/g)]
  .map((match) => match[1])
  .filter((value) => !value.startsWith("/") && !value.includes(":") && value !== "#");

// This preserves strings but does not parse comment markers in template interpolations.
const stripComments = (source) => {
  const strings = [];
  const withoutStrings = source.replace(/(["'`])(?:\\.|(?!\1)[\s\S])*?\1/g, (match) => {
    strings.push(match);
    return `__STRING_${strings.length - 1}__`;
  });
  const withoutComments = withoutStrings
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/^\s*#.*$/gm, "")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\/\/[^\n]*/g, "");
  return withoutComments.replace(/__STRING_(\d+)__/g, (_, index) => strings[Number(index)]);
};

const iconRule = (source, icon) => source.match(
  new RegExp(`\\.ph\\.${icon}:before\\s*\\{[^}]*\\}`, "m")
)?.[0];
const validateIconCss = (source, usedIcons) => {
  assert.match(source, /\.ph\s*\{[^}]*font-family:\s*["']Phosphor["']/s,
    "phosphor.css must retain the base .ph rule");
  assert.match(source, /@font-face\s*\{[^}]*font-family:\s*["']Phosphor["'][^}]*\}/s,
    "phosphor.css must retain the Phosphor font face");
  for (const icon of usedIcons) assert.ok(iconRule(source, icon), `used icon ${icon} is absent from phosphor.css`);
};

const fontFaces = (source) => [...source.matchAll(/@font-face\s*\{([\s\S]*?)\}/g)].flatMap((match) => {
  const family = match[1].match(/font-family:\s*["']([^"']+)["']/)?.[1];
  const url = match[1].match(/src:\s*url\(["']?([^"')]+)["']?\)/)?.[1];
  return family && url ? [{ family, url, block: match[0] }] : [];
});
const usedFontFamilies = (source) => new Set([...source.replace(/@font-face\s*\{[\s\S]*?\}/g, "")
  .matchAll(/font-family:\s*["']([^"']+)/g)].map((match) => match[1]));

test("keeps JavaScript DOM hooks and CSS-toggled classes backed by settings markup", () => {
  const settingsIds = new Set([...ids(settingsHtml), ...dynamicIds(settingsJs), ...markupIds(settingsJs)]);
  const selectorIds = new Set([...settingsJs.matchAll(/(?:getElementById\(\s*["']|querySelector\(\s*["']#)([\w-]+)/g)]
    .map((match) => match[1]));
  assert.deepEqual(sorted(selectorIds), expectedSelectorIds);
  for (const id of selectorIds) assert.ok(settingsIds.has(id), `settings.js references missing #${id}`);

  const toggledClasses = new Set([...settingsJs.matchAll(/classList\.(?:add|remove|toggle)\(\s*["']([\w-]+)/g)]
    .map((match) => match[1]));
  const toggleTargets = {
    current: "wifi-network-name", disconnected: "wifi-current-ssid", hidden: "accessibility-banner",
    recording: "hotkey-recorder",
  };
  assert.deepEqual(sorted(toggledClasses), Object.keys(toggleTargets).sort());
  for (const className of toggledClasses) {
    assert.match(css["src/settings.css"], new RegExp(`\\.${className}(?:\\b|[.#[:])`), `missing CSS for .${className}`);
    const target = toggleTargets[className];
    assert.ok(settingsIds.has(target) || dynamicClasses(settingsJs).has(target),
      `.${className} needs its ${target} target`);
  }
});

test("keeps frontend translation keys defined in English and every shipped locale", () => {
  const keys = new Set();
  for (const source of Object.values(html)) for (const key of attributeValues(source, "data-i18n")) keys.add(key);
  for (const source of Object.values(js)) for (const key of source.matchAll(/(?:data-i18n["']?\s*,?\s*["']|key:\s*["'])([\w-]+)/g)) keys.add(key[1]);
  assert.deepEqual(sorted(keys), expectedTranslationKeys);

  const localeDir = join(root, "src-tauri/locales");
  const english = fluentKeys(read("src-tauri/locales/en.ftl"));
  for (const key of keys) assert.ok(english.has(key), `English locale is missing ${key}`);
  for (const locale of readdirSync(localeDir).filter((name) => name.endsWith(".ftl"))) {
    assert.deepEqual(fluentKeys(read(`src-tauri/locales/${locale}`)), english, `${locale} key set differs from English`);
  }
});

test("keeps frontend IPC calls registered by Tauri", () => {
  const commands = new Set(Object.values(js).flatMap((source) => [...commandNames(source)]));
  assert.deepEqual(sorted(commands), expectedCommands);
  const registered = registeredCommands(read("src-tauri/src/lib.rs"));
  for (const command of commands) assert.ok(registered.has(command), `frontend invokes unregistered Tauri command ${command}`);
});

test("keeps bundled pages, local resources, CSP, and used icons available", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const csp = config.app.security.csp;
  assert.equal(csp, expectedCsp);
  for (const directive of ["style-src 'self'", "font-src 'self'", "script-src 'self'", "img-src 'self'"]) {
    assert.ok(csp.includes(directive), `CSP must permit local ${directive.split("-")[0]} resources`);
  }

  assert.deepEqual(config.app.windows, [], "Tauri windows are created dynamically");
  const dynamicPages = [...read("src-tauri/src/lib.rs").matchAll(/WebviewUrl::App\("([^"\\]+)"/g)]
    .map((match) => match[1]);
  assert.deepEqual(sorted(dynamicPages), ["about.html", "settings.html"]);
  for (const page of dynamicPages) assert.ok(existsSync(join(root, "src", page)), `missing dynamic Tauri page ${page}`);

  const resources = new Set(Object.values(html).flatMap(resourcePaths));
  assert.deepEqual(sorted(resources), expectedResources);
  for (const resource of resources) assert.ok(existsSync(join(root, "src", resource)), `missing frontend resource ${resource}`);

  const faces = Object.entries(css).flatMap(([path, source]) => fontFaces(source).map((face) => ({ ...face, path })));
  const consumers = new Set(Object.values(css).flatMap((source) => [...usedFontFamilies(source)]));
  assert.ok(consumers.has("Geist"), "Geist must remain used by shipped CSS");
  assert.ok(consumers.has("Phosphor"), "Phosphor must remain used by shipped CSS");
  for (const face of faces.filter((face) => face.family !== "Inter")) {
    const target = join(root, dirname(face.path), face.url);
    assert.ok(existsSync(target), `${face.path} references missing ${face.url}`);
  }
  assert.ok(csp.includes("font-src 'self'"), "CSP must permit shipped fonts");
  const inter = faces.find((face) => face.family === "Inter");
  // Characterization only: Inter is removable, so its later absence remains valid.
  if (inter) {
    assert.ok(!consumers.has("Inter"), "Inter must not gain a consumer before its removal");
    assert.ok(!existsSync(join(root, dirname(inter.path), inter.url)), "Inter's unused file must stay absent");
  }

  const usedIcons = new Set([...Object.values(html), ...Object.values(js)].flatMap((source) => [...iconClasses(source)]));
  assert.deepEqual(sorted(usedIcons), expectedIcons);
  validateIconCss(phosphorCss, usedIcons);
  const trimmedCss = [
    phosphorCss.match(/@font-face\s*\{[^}]*\}/s)?.[0],
    phosphorCss.match(/\.ph\s*\{[^}]*\}/s)?.[0],
    ...expectedIcons.map((icon) => iconRule(phosphorCss, icon)),
  ].join("\n");
  validateIconCss(trimmedCss, usedIcons);
  assert.throws(() => validateIconCss(trimmedCss.replace(iconRule(trimmedCss, "ph-x"), ""), usedIcons),
    /ph-x/, "the trimming validator must reject a missing used icon");

  assert.match(stripComments(settingsJs), /\bph-x\b/,
    "settings.js retains the literal ph-x icon contract");
  for (const [path, source] of Object.entries(js)) {
    const uncommented = stripComments(source);
    assert.doesNotMatch(uncommented, /["'`]ph-\s*(?:["'`+]|\$\{)/,
      `${path} dynamically assembles an icon class`);
    assert.doesNotMatch(uncommented, /\[\s*["']ph["']\s*,[\s\S]*?\]\s*\.join\(\s*["']-["']\s*\)/,
      `${path} dynamically joins an icon class`);
  }
});
