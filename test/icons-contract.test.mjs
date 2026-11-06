/**
 * @spec-handoff
 * @interface iconAssets(): void
 * @behavior
 *   - Ships the mapped local SVG icons and uses their geometry inline in Settings.
 *   - Allows only fixed tray-menu emoji glyphs in the Rust tray module.
 *   - Removes every other product emoji while preserving localized accessible names.
 *   - Keeps decorative icons hidden from assistive technology and CSP local-only.
 * @edge-cases
 *   - Dynamic network removal names include the SSID without HTML interpolation.
 *   - SVG markup is safe, color-inheriting, and within the icon byte budgets.
 * @see ../src/settings.html
 */

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { stripHtmlComments } from "./helpers/strip-html-comments.mjs";

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
  ? stripHtmlComments(source)
  : source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
const svgMarkup = (source) => source.match(/<svg\b[^>]*>[\s\S]*?<\/svg>/gi) ?? [];
const canonicalSvg = (source) => normalize(source
  .replace(/\s(?:aria-hidden|focusable|class)=(?:"[^"]*"|'[^']*')/gi, ""));
const asset = (name) => read(`src/icons/${name}.svg`);
const assetGeometry = (name) => canonicalSvg(asset(name));
const hasAssetGeometry = (source, name) => svgMarkup(source)
  .some((svg) => canonicalSvg(svg) === assetGeometry(name));
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
const trayMenuEmoji = ["🟢", "⚪", "📅", "⏸️", "⚠️"];
const trayMenuEmojiEscapes = [
  String.raw`\u{1f7e2}`, String.raw`\u{26aa}`, String.raw`\u{1f4c5}`,
  String.raw`\u{23f8}\u{fe0f}`, String.raw`\u{26a0}\u{fe0f}`,
];
const trayMenuEmojiConstants = new Map([
  ["TRAY_ACTIVE_EMOJI", "🟢"], ["TRAY_INACTIVE_EMOJI", "⚪"],
  ["TRAY_WAITING_EMOJI", "📅"], ["TRAY_PAUSED_EMOJI", "⏸️"],
]);
const originalTrayAssets = new Map([
  ["tray-icon.png", { bytes: 342, sha256: "36e5eebd2f16f6676753cce512d51c4b60d42edc5728c39717503cf0061de397" }],
  ["tray-icon@2x.png", { bytes: 621, sha256: "7adbbbd3a037affb93c8e3a85c3ebf3da584c98f2f5c5d1779fd726a02a82d8d" }],
  ["tray-icon-dark.png", { bytes: 573, sha256: "b28a25e292bb8c4e394bdf83b52d0286e9c92789044da8ad84526455180a2b9c" }],
]);

// Mutant: replace an original asset, retain a silhouette, or restore template rendering.
test("restores the original dimmable tray assets and removes silhouette assets", () => {
  const iconDirectory = join(root, "src-tauri/icons");
  for (const [name, expected] of originalTrayAssets) {
    const asset = readFileSync(join(iconDirectory, name));
    assert.equal(asset.length, expected.bytes, `${name} byte length`);
    assert.equal(createHash("sha256").update(asset).digest("hex"), expected.sha256,
      `${name} SHA-256`);
  }
  for (const name of [
    "tray-active.png", "tray-active@2x.png", "tray-inactive.png", "tray-inactive@2x.png",
    "tray-waiting.png", "tray-waiting@2x.png", "tray-linux-active.png",
    "tray-linux-inactive.png", "tray-linux-waiting.png",
  ]) {
    assert.equal(existsSync(join(iconDirectory, name)), false, `${name} must be removed`);
  }
  const rust = read("src-tauri/src/lib.rs")
    .split("\n#[cfg(test)]\nmod tray_icon_contract_tests")[0];
  assert.doesNotMatch(rust, /icon_as_template|set_icon_with_as_template/);
  assert.match(rust, /self\.tray\.set_icon\(Some\(image\)\)/);
});

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

// Mutant: restore a Phosphor class or alter an inline SVG element's geometry attributes.
test("uses each mapped Settings icon as canonical inline SVG elements without Phosphor classes", () => {
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

// Mutant: add a non-tray emoji or move an allowed tray glyph into another source file.
test("allows only fixed tray-menu glyphs in lib.rs and no other product emoji", () => {
  for (const path of productFiles) {
    let source = stripComments(readFileSync(path, "utf8"),
      path.endsWith(".html") ? ".html" : ".code");
    if (relative(root, path) === "src-tauri/src/lib.rs") {
      for (const glyph of trayMenuEmoji) source = source.replaceAll(glyph, "");
      for (const escapedGlyph of trayMenuEmojiEscapes) {
        source = source.replaceAll(escapedGlyph, "");
      }
    }
    assert.equal(emoji.test(source), false, `${relative(root, path)} contains a product emoji`);
    assert.equal(emojiEscape.test(source), false,
      `${relative(root, path)} contains an escaped product emoji`);
  }
  const lib = read("src-tauri/src/lib.rs");
  for (const [name, glyph] of trayMenuEmojiConstants) {
    assert.match(lib, new RegExp(`const\\s+${name}\\s*:\\s*&str\\s*=\\s*"${glyph}"`));
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
