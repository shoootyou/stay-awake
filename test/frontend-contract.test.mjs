/**
 * @spec-handoff
 * @interface frontendContracts(): void
 * @behavior
 *   - Keeps frontend hooks, translations, IPC commands, resources, icons, and pages aligned.
 *   - Keeps the current CSP suitable for local scripts, styles, fonts, and images.
 * @edge-cases
 *   - Every shipped locale has the English key set.
 *   - Dynamically rendered icon names must be listed explicitly before CSS is trimmed.
 * @see ../src-tauri/src/lib.rs
 */

import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const read = (path) => readFileSync(join(root, path), "utf8");
const htmlPaths = ["src/index.html", "src/settings.html", "src/about.html"];
const jsPaths = ["src/settings.js", "src/about.js"];
const html = Object.fromEntries(htmlPaths.map((path) => [path, read(path)]));
const js = Object.fromEntries(jsPaths.map((path) => [path, read(path)]));
const settingsHtml = html["src/settings.html"];
const settingsJs = js["src/settings.js"];

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

test("keeps JavaScript DOM hooks and CSS-toggled classes backed by settings markup", () => {
  const settingsIds = new Set([...ids(settingsHtml), ...dynamicIds(settingsJs), ...markupIds(settingsJs)]);
  const selectorIds = new Set([
    ...settingsJs.matchAll(/(?:getElementById\(\s*["']|querySelector\(\s*["']#)([\w-]+)/g),
  ].map((match) => match[1]));

  for (const id of selectorIds) {
    assert.ok(settingsIds.has(id), `settings.js references missing #${id}`);
  }

  const toggledClasses = new Set([...settingsJs.matchAll(/classList\.(?:add|remove|toggle)\(\s*["']([\w-]+)["']/g)]
    .map((match) => match[1]));
  const css = read("src/settings.css");
  const toggleTargets = {
    current: "wifi-network-name",
    disconnected: "wifi-current-ssid",
    hidden: "accessibility-banner",
    recording: "hotkey-recorder",
  };
  assert.deepEqual([...toggledClasses].sort(), Object.keys(toggleTargets).sort());
  for (const className of toggledClasses) {
    assert.match(css, new RegExp(`\\.${className}(?:\\b|[.#[:])`), `missing CSS for .${className}`);
    const target = toggleTargets[className];
    assert.ok(settingsIds.has(target) || dynamicClasses(settingsJs).has(target),
      `.${className} needs its ${target} target`);
  }
});

test("keeps frontend translation keys defined in English and every shipped locale", () => {
  const keys = new Set();
  for (const source of Object.values(html)) {
    for (const key of attributeValues(source, "data-i18n")) keys.add(key);
  }
  for (const source of Object.values(js)) {
    for (const key of [...source.matchAll(/(?:data-i18n["']?\s*,?\s*["']|key:\s*["'])([\w-]+)/g)]) {
      keys.add(key[1]);
    }
  }

  const localeDir = join(root, "src-tauri/locales");
  const english = fluentKeys(read("src-tauri/locales/en.ftl"));
  for (const key of keys) assert.ok(english.has(key), `English locale is missing ${key}`);
  for (const locale of readdirSync(localeDir).filter((name) => name.endsWith(".ftl"))) {
    assert.deepEqual(fluentKeys(read(`src-tauri/locales/${locale}`)), english, `${locale} key set differs from English`);
  }
});

test("keeps frontend IPC calls registered by Tauri", () => {
  const commands = new Set(Object.values(js).flatMap((source) => [...commandNames(source)]));
  const registered = registeredCommands(read("src-tauri/src/lib.rs"));
  for (const command of commands) {
    assert.ok(registered.has(command), `frontend invokes unregistered Tauri command ${command}`);
  }
});

test("keeps bundled pages, local resources, CSP, and used icons available", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const csp = "default-src 'self'; style-src 'self'; font-src 'self'; script-src 'self'; img-src 'self' asset: https://asset.localhost; connect-src ipc: http://ipc.localhost https://github.com https://api.github.com";
  assert.equal(config.app.security.csp, csp);
  for (const directive of ["style-src 'self'", "font-src 'self'", "script-src 'self'", "img-src 'self'"]) {
    assert.ok(csp.includes(directive), `CSP must permit local ${directive.split("-")[0]} resources`);
  }

  for (const page of config.app.windows) {
    if (typeof page.url === "string" && !page.url.includes("://")) {
      assert.ok(existsSync(join(root, "src", page.url.replace(/^\//, ""))), `missing configured page ${page.url}`);
    }
  }
  const dynamicPages = [...read("src-tauri/src/lib.rs").matchAll(/WebviewUrl::App\("([^"\\]+)"/g)]
    .map((match) => match[1]);
  for (const page of dynamicPages) {
    assert.ok(existsSync(join(root, "src", page)), `missing dynamic Tauri page ${page}`);
  }

  for (const [path, source] of Object.entries(html)) {
    for (const resource of resourcePaths(source)) {
      assert.ok(existsSync(join(root, "src", resource)), `${path} references missing ${resource}`);
    }
  }
  for (const [cssPath, fontPath] of [["src/settings.css", "src/fonts/Geist.woff2"], ["src/fonts/phosphor.css", "src/fonts/Phosphor.woff2"]]) {
    assert.ok(read(cssPath).includes("url("), `${cssPath} must retain its local font reference`);
    assert.ok(existsSync(join(root, fontPath)), `missing runtime font ${fontPath}`);
  }

  const usedIcons = new Set([...Object.values(html), ...Object.values(js)].flatMap((source) => [...iconClasses(source)]));
  const definedIcons = iconClasses(read("src/fonts/phosphor.css"));
  for (const icon of usedIcons) assert.ok(definedIcons.has(icon), `used icon ${icon} is absent from phosphor.css`);
  assert.doesNotMatch(settingsJs, /ph-\s*['"`+]/, "dynamically assembled icon names need an explicit contract");
});
