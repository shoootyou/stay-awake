/**
 * @spec-handoff
 * @interface iconAssets(): void
 * @behavior
 *   - Ships the mapped local SVG icons and uses their geometry inline in Settings.
 *   - Removes Phosphor and product emoji while preserving localized accessible names.
 *   - Keeps decorative icons hidden from assistive technology and CSP local-only.
 * @edge-cases
 *   - Dynamic network removal names include the SSID without HTML interpolation.
 *   - SVG markup is safe, color-inheriting, and within the icon byte budgets.
 * @see ../src/settings.html
 */

import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const read = (path) => readFileSync(join(root, path), "utf8");
const iconDir = join(root, "src/icons");
const iconNames = [
  "gear", "cursor-click", "info", "timer", "app-window", "power", "wifi-high",
  "translate", "keyboard", "calendar-blank", "user-circle", "x", "play",
  "check-circle", "warning", "map-pin", "heart",
];
const settingsIcons = new Map([
  ["gear", "settings-title"], ["cursor-click", "settings-mode-label"],
  ["info", "mode-info-btn"], ["timer", "settings-interval-label"],
  ["app-window", "settings-app-mode-label"], ["power", "settings-autostart-label"],
  ["wifi-high", "settings-wifi-title"], ["translate", "settings-language-label"],
  ["keyboard", "settings-hotkey-label"], ["calendar-blank", "schedule-label"],
  ["user-circle", "profile-label"],
]);
const expectedCsp = "default-src 'self'; style-src 'self'; font-src 'self'; script-src 'self'; "
  + "img-src 'self' asset: https://asset.localhost; connect-src ipc: http://ipc.localhost "
  + "https://github.com https://api.github.com";

const normalize = (value) => value.replaceAll(/\s+/g, " ").trim();
const stripComments = (source, extension) => extension === ".html"
  ? source.replace(/<!--[\s\S]*?-->/g, "")
  : source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
const svgMarkup = (source) => source.match(/<svg\b[^>]*>[\s\S]*?<\/svg>/gi) ?? [];
const geometry = (source) => normalize(source.replace(/<svg\b[^>]*>|<\/svg>/gi, ""));
const asset = (name) => read(`src/icons/${name}.svg`);
const assetGeometry = (name) => geometry(asset(name));
const hasAssetGeometry = (source, name) => svgMarkup(source)
  .some((svg) => geometry(svg) === assetGeometry(name));
const allFiles = (directory) => readdirSync(directory, { recursive: true })
  .map((entry) => join(directory, entry)).filter((path) => statSync(path).isFile());
const productFiles = [
  ...allFiles(join(root, "src")).filter((path) => /\.(?:html|js|css|svg)$/u.test(path)),
  ...allFiles(join(root, "src-tauri/src")).filter((path) => path.endsWith(".rs")),
  ...allFiles(join(root, "src-tauri/locales")).filter((path) => path.endsWith(".ftl")),
];
const emoji = /[\u{1F300}-\u{1FAFF}\u{2600}-\u{27BF}\u{FE0F}\u{2764}\u{2665}]/u;
const emojiEscape = new RegExp([
  String.raw`\\u\{(?:1F[3-9A-Fa-f][0-9A-Fa-f]{2}|260[0-9A-Fa-f]|27[0-9A-Fa-f]{2}|FE0F|2764|2665)\}`,
  String.raw`\\u(?:D83[C-F]|D8[0-9A-Fa-f]{2}|D[CE][0-9A-Fa-f]{2}|260[0-9A-Fa-f]|27[0-9A-Fa-f]{2}|FE0F|2764|2665)`,
].join("|"), "u");

// Mutant: delete one asset, add an executable SVG element, or hard-code a fill color.
test("ships safe canonical regular SVG assets with attribution inside the byte budget", () => {
  assert.ok(existsSync(join(iconDir, "LICENSE")),
    "src/icons/LICENSE must preserve icon attribution");
  let bytes = 0;
  for (const name of iconNames) {
    const path = join(iconDir, `${name}.svg`);
    assert.ok(existsSync(path), `missing canonical ${name}.svg`);
    bytes += statSync(path).size;
    const source = readFileSync(path, "utf8");
    assert.match(source, /<svg\b[^>]*\bviewBox=["'][^"']+["']/i, `${name} needs a viewBox`);
    assert.ok(!/\b(?:fill|stroke)=["'](?!none|currentColor)[^"']+/i.test(source),
      `${name} hard-codes color`);
    assert.doesNotMatch(source, /<script\b|\bon[a-z]+\s*=/i,
      `${name} contains executable SVG markup`);
    assert.doesNotMatch(source, /<image\b|\b(?:href|xlink:href)=["'](?:https?:|\/\/)/i,
      `${name} loads a remote or raster resource`);
    assert.doesNotMatch(source, /@font-face|font-family/i, `${name} depends on a font`);
  }
  assert.ok(bytes <= 32_768, `SVG assets use ${bytes} bytes; maximum is 32768`);
});

// Mutant: restore a Phosphor class or replace an inline path with a different geometry.
test("uses each mapped Settings icon as canonical inline SVG geometry without Phosphor classes", () => {
  const settingsHtml = read("src/settings.html");
  const settingsJs = stripComments(read("src/settings.js"), ".js");
  assert.doesNotMatch(`${settingsHtml}\n${settingsJs}`,
    /<i\b[^>]*\bclass=["'][^"']*\bph-|\bph-[\w-]+/i);
  for (const [name, marker] of settingsIcons) {
    const markerIndex = settingsHtml.indexOf(marker);
    assert.notEqual(markerIndex, -1, `Settings is missing ${marker}`);
    const scopeStart = marker === "settings-title"
      ? settingsHtml.lastIndexOf("<header", markerIndex)
      : marker === "mode-info-btn"
        ? settingsHtml.lastIndexOf("<button", markerIndex)
        : Math.max(settingsHtml.lastIndexOf('class="label"', markerIndex),
          settingsHtml.lastIndexOf('class="group-header"', markerIndex));
    const scopeEnd = marker === "mode-info-btn"
      ? settingsHtml.indexOf("</button>", markerIndex) + "</button>".length
      : markerIndex;
    assert.ok(scopeStart >= 0 && hasAssetGeometry(settingsHtml.slice(scopeStart, scopeEnd), name),
      `${marker} must use ${name}.svg geometry inline`);
  }
  assert.ok(hasAssetGeometry(settingsJs, "x"), "network removal must use x.svg geometry inline");
});

// Mutant: restore the removed font file, link, dependency, or a Phosphor font face.
test("removes Phosphor while retaining Geist and local-only frontend resources", () => {
  const packageJson = JSON.parse(read("package.json"));
  const lockfile = read("package-lock.json");
  const frontend = allFiles(join(root, "src")).filter((path) => /\.(?:html|js|css)$/u.test(path))
    .map((path) => readFileSync(path, "utf8")).join("\n");
  assert.equal(existsSync(join(root, "src/fonts/Phosphor.woff2")), false);
  assert.equal(existsSync(join(root, "src/fonts/phosphor.css")), false);
  assert.doesNotMatch(frontend,
    /Phosphor|phosphor\.css|@font-face[^}]*font-family:\s*["']Phosphor/i);
  assert.equal(packageJson.dependencies?.["@phosphor-icons/web"], undefined);
  assert.doesNotMatch(lockfile, /node_modules\/@phosphor-icons\/web|"@phosphor-icons\/web"/);
  assert.ok(existsSync(join(root, "src/fonts/Geist.woff2")), "Geist font file must remain");
  assert.match(frontend, /font-family:\s*["']Geist/i, "Geist must remain used");
});

// Mutant: reintroduce a product emoji, including an escaped Unicode spelling.
test("contains no product emoji in shipped frontend, Rust, or locale sources", () => {
  for (const path of productFiles) {
    const source = stripComments(readFileSync(path, "utf8"),
      path.endsWith(".html") ? ".html" : ".code");
    assert.equal(emoji.test(source), false, `${relative(root, path)} contains a product emoji`);
    assert.equal(emojiEscape.test(source), false,
      `${relative(root, path)} contains an escaped product emoji`);
  }
});

// Mutant: remove aria-hidden, desynchronize Info state, or insert the SSID through HTML.
test("keeps icon accessibility semantics and localized Info and network removal names", () => {
  const settingsHtml = read("src/settings.html");
  const settingsJs = stripComments(read("src/settings.js"), ".js");
  const aboutHtml = read("src/about.html");
  const infoButton = settingsHtml
    .match(/<button\b[^>]*\bid=["']mode-info-btn["'][^>]*>/i)?.[0] ?? "";
  assert.match(infoButton, /data-i18n-aria-label=["']settings-mode-info-label["']/);
  assert.match(infoButton, /aria-controls=["']mode-info-panel["']/);
  assert.match(infoButton, /aria-expanded=["']false["']/);
  const translations = settingsJs.match(/async function applyTranslations\(\)[\s\S]*?\n\}/)?.[0]
    ?? "";
  assert.match(translations,
    /data-i18n-aria-label[\s\S]*?setAttribute\(\s*["']aria-label["']\s*,\s*text\s*\)/);
  assert.match(settingsJs, /mode-info-btn[\s\S]*?setAttribute\(\s*["']aria-expanded["']/);
  assert.match(settingsJs,
    /get_translation[\s\S]*settings-wifi-remove-label[\s\S]*network\s*:\s*ssid/);
  assert.match(settingsJs, /setAttribute\(\s*["']aria-label["']/);
  assert.doesNotMatch(settingsJs, /(?:innerHTML|insertAdjacentHTML)[\s\S]{0,160}\$\{ssid\}/);
  const iconMarkup = [...svgMarkup(settingsHtml), ...svgMarkup(settingsJs), ...svgMarkup(aboutHtml)];
  for (const svg of iconMarkup) {
    assert.match(svg, /aria-hidden=["']true["']/);
    assert.match(svg, /focusable=["']false["']/);
    assert.doesNotMatch(svg, /\btabindex\b/i);
  }
  assert.match(aboutHtml,
    /<svg\b[\s\S]*?aria-hidden=["']true["'][\s\S]*?<\/svg>\s*<span[^>]*class=["']sr-only["']>love<\/span>/i);
  assert.doesNotMatch(settingsHtml, /data-i18n=["'][^"']+["'][^>]*>[^<]*<svg\b/i);
});

// Mutant: change CSP or introduce a remote SVG/image URL.
test("keeps the CSP unchanged, icons local, and icon markup within its byte budget", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const frontend = ["src/settings.html", "src/settings.js", "src/settings.css", "src/about.html", "src/about.css"]
    .map(read).join("\n");
  const inlineIconBytes = Buffer.byteLength(svgMarkup(frontend).join(""))
    + Buffer.byteLength((read("src/settings.css")
      .match(/[^{}]*\b(?:icon|svg)\b[^{}]*\{[^}]*\}/gi) ?? []).join(""));
  assert.equal(config.app.security.csp, expectedCsp);
  assert.doesNotMatch(frontend,
    /<svg\b[^>]*(?:href|src)=["']https?:|<use\b[^>]*(?:href|xlink:href)=["']https?:/i);
  assert.ok(inlineIconBytes <= 32_768,
    `inline icon markup and CSS use ${inlineIconBytes} bytes; maximum is 32768`);
});
