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
  .map((path) => `src/${path.replaceAll("\\", "/")}`);
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
  "profile-delete", "profile-label", "profile-save-as", "schedule-days", "schedule-enabled", "schedule-enabled-hint",
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
const handlerEntries = (source) => {
  const uncommented = stripComments(source, "rust");
  const match = uncommented.match(/generate_handler!\s*\[/);
  if (!match || match.index === undefined) return "";
  const open = match.index + match[0].length - 1;
  let depth = 0;
  for (let index = open; index < uncommented.length; index += 1) {
    if (uncommented[index] === "[") depth += 1;
    if (uncommented[index] === "]" && --depth === 0) return uncommented.slice(open + 1, index);
  }
  return "";
};
const registeredCommands = (source) => {
  const handler = handlerEntries(source);
  const entries = handler.replace(/#\[[^\]]*\]\s*/g, "");
  return new Set([...entries.matchAll(/^\s*([a-z][a-z0-9_]*)\s*,/gm)].map((match) => match[1]));
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

// This handles common JavaScript regex starts; ambiguity after ) or ] and nested template interpolation remain unsupported.
const stripComments = (source, language = "js") => {
  let state = "code";
  let blockCommentDepth = 0;
  let regexClass = false;
  let token = "";
  let previousToken = null;
  let output = "";
  const finishToken = () => {
    if (token) previousToken = token;
    token = "";
  };
  const recordCodeCharacter = (character) => {
    if (/[\w$]/.test(character)) {
      token += character;
    } else if (!/\s/.test(character)) {
      finishToken();
      previousToken = character;
    } else {
      finishToken();
    }
  };
  const beginsRegex = () => previousToken === null
    || "(,=:[!&|?{};".includes(previousToken)
    || ["return", "typeof", "case", "in", "of", "delete", "void", "yield", "await", "throw", "new", "else", "do"].includes(previousToken);
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];
    if (state === "lineComment") {
      output += character === "\n" ? "\n" : " ";
      if (character === "\n") state = "code";
      continue;
    }
    if (state === "blockComment") {
      output += character === "\n" ? "\n" : " ";
      if (language === "rust" && character === "/" && next === "*") {
        output += " ";
        index += 1;
        blockCommentDepth += 1;
      } else if (character === "*" && next === "/") {
        output += " ";
        index += 1;
        blockCommentDepth -= 1;
        if (blockCommentDepth === 0) state = "code";
      }
      continue;
    }
    if (state === "single" || state === "double" || state === "template" || state === "regex") {
      output += character;
      if (character === "\\") {
        output += next ?? "";
        index += 1;
      } else if (state === "regex" && character === "[") {
        regexClass = true;
      } else if (state === "regex" && character === "]") {
        regexClass = false;
      } else if (state === "regex" && character === "/" && !regexClass) {
        state = "code";
        previousToken = "literal";
      } else if ((state === "single" && character === "'")
        || (state === "double" && character === '"')
        || (state === "template" && character === "`")) {
        state = "code";
        previousToken = "literal";
      }
      continue;
    }
    if (language === "rust" && character === "r") {
      const rawString = /^r(#+)?"/.exec(source.slice(index));
      if (rawString) {
        const hashes = rawString[1] ?? "";
        const end = source.indexOf(`"${hashes}`, index + rawString[0].length);
        if (end !== -1) {
          const raw = source.slice(index, end + hashes.length + 1);
          output += raw;
          index += raw.length - 1;
          continue;
        }
      }
    }
    if (character === "/" && next === "/") {
      output += "  ";
      index += 1;
      state = "lineComment";
    } else if (character === "/" && next === "*") {
      output += "  ";
      index += 1;
      state = "blockComment";
      blockCommentDepth = 1;
    } else if (language === "js" && character === "/" && beginsRegex()) {
      finishToken();
      output += character;
      state = "regex";
      regexClass = false;
    } else {
      output += character;
      if (character === "'" && (language === "js" || /^'(?:[^\\'\n]|\\(?:[nrt0\\'\"]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]+\}))'/.test(source.slice(index)))) {
        finishToken();
        state = "single";
      } else if (character === '"') {
        finishToken();
        state = "double";
      } else if (language === "js" && character === "`") {
        finishToken();
        state = "template";
      } else {
        recordCodeCharacter(character);
      }
    }
  }
  return output;
};

const stripHtmlComments = (source) => source.replace(/<!--[\s\S]*?-->/g, (comment) => comment.replace(/[^\n]/g, " "));
const stripCssComments = (source) => source.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "));
const stripFtlComments = (source) => source.replace(/^\s*#.*$/gm, (comment) => " ".repeat(comment.length));
const incompleteIconFragments = (source, allowedIcons) => {
  const fragments = new Set([...source.matchAll(/\bph-[a-z0-9-]*/g)]
    .map((match) => match[0])
    .filter((icon) => !allowedIcons.has(icon)));
  if (/["']ph["']\s*\+\s*["']-["']/.test(source)) fragments.add("ph + -");
  if (/\[\s*["']ph["']\s*,[\s\S]*?\]\s*\.join\(\s*["']-["']\s*\)/.test(source)) {
    fragments.add("ph.join(-)");
  }
  return fragments;
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
  const uncommentedSettingsJs = stripComments(settingsJs);
  const settingsIds = new Set([...ids(stripHtmlComments(settingsHtml)), ...dynamicIds(uncommentedSettingsJs), ...markupIds(uncommentedSettingsJs)]);
  const selectorIds = new Set([...uncommentedSettingsJs.matchAll(/(?:getElementById\(\s*["']|querySelector\(\s*["']#)([\w-]+)/g)]
    .map((match) => match[1]));
  assert.deepEqual(sorted(selectorIds), expectedSelectorIds);
  for (const id of selectorIds) assert.ok(settingsIds.has(id), `settings.js references missing #${id}`);

  const toggledClasses = new Set([...uncommentedSettingsJs.matchAll(/classList\.(?:add|remove|toggle)\(\s*["']([\w-]+)/g)]
    .map((match) => match[1]));
  const toggleTargets = {
    current: "wifi-network-name", disconnected: "wifi-current-ssid", hidden: "accessibility-banner",
    recording: "hotkey-recorder",
  };
  assert.deepEqual(sorted(toggledClasses), Object.keys(toggleTargets).sort());
  for (const className of toggledClasses) {
    assert.match(css["src/settings.css"], new RegExp(`\\.${className}(?:\\b|[.#[:])`), `missing CSS for .${className}`);
    const target = toggleTargets[className];
    assert.ok(settingsIds.has(target) || dynamicClasses(uncommentedSettingsJs).has(target),
      `.${className} needs its ${target} target`);
  }
});

test("keeps frontend translation keys defined in English and every shipped locale", () => {
  const keys = new Set();
  for (const source of Object.values(html)) for (const key of attributeValues(stripHtmlComments(source), "data-i18n")) keys.add(key);
  for (const source of Object.values(js)) for (const key of stripComments(source).matchAll(/(?:data-i18n["']?\s*,?\s*["']|key:\s*["'])([\w-]+)/g)) keys.add(key[1]);
  assert.deepEqual(sorted(keys), expectedTranslationKeys);

  const localeDir = join(root, "src-tauri/locales");
  const english = fluentKeys(stripFtlComments(read("src-tauri/locales/en.ftl")));
  for (const key of keys) assert.ok(english.has(key), `English locale is missing ${key}`);
  for (const locale of readdirSync(localeDir).filter((name) => name.endsWith(".ftl"))) {
    assert.deepEqual(fluentKeys(stripFtlComments(read(`src-tauri/locales/${locale}`))), english, `${locale} key set differs from English`);
  }
});

test("keeps frontend IPC calls registered by Tauri", () => {
  const commands = new Set(Object.values(js).flatMap((source) => [...commandNames(stripComments(source))]));
  assert.deepEqual(sorted(commands), expectedCommands);
  const registered = registeredCommands(read("src-tauri/src/lib.rs"));
  for (const command of commands) assert.ok(registered.has(command), `frontend invokes unregistered Tauri command ${command}`);
});

test("does not treat commented Rust handler entries as registrations", () => {
  const source = "generate_handler![\n  // update_global_hotkey,\n  /// update_global_hotkey\n  //! update_global_hotkey\n  /* update_global_hotkey */\n]";

  assert.equal(registeredCommands(source).has("update_global_hotkey"), false);
});

test("ignores handler entry attributes without registering absent commands", () => {
  const source = "generate_handler![\n  #[cfg(desktop)]\n  get_config,\n]";
  const commands = registeredCommands(source);

  assert.equal(commands.has("get_config"), true);
  assert.equal(commands.has("save_config"), false);
});

test("ignores nested Rust block comments when extracting handler registrations", () => {
  const source = "generate_handler![/* a /* update_global_hotkey, */ c */ update_global_hotkey,]";

  assert.equal(registeredCommands(source).has("update_global_hotkey"), true);
});

test("does not let Rust lifetimes, raw strings, or comments register handler entries", () => {
  const source = `fn borrow(value: &'_ str) { let raw = r#"// not a comment /* either */"#; }
generate_handler![
  /*
    update_global_hotkey,
  */
]`;

  assert.equal(registeredCommands(source).has("update_global_hotkey"), false);
});

test("keeps bundled pages, local resources, CSP, and used icons available", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const csp = config.app.security.csp;
  assert.equal(csp, expectedCsp);
  assert.deepEqual(config.app.windows, [], "Tauri windows are created dynamically");
  const dynamicPages = [...read("src-tauri/src/lib.rs").matchAll(/WebviewUrl::App\("([^"\\]+)"/g)]
    .map((match) => match[1]);
  assert.deepEqual(sorted(dynamicPages), ["about.html", "settings.html"]);
  for (const page of dynamicPages) assert.ok(existsSync(join(root, "src", page)), `missing dynamic Tauri page ${page}`);

  const scripts = new Set(Object.values(html).flatMap((source) => [...stripHtmlComments(source).matchAll(
    /<script[^>]+src=["']\/?([^"']+\.js)["']/g
  )].map((match) => match[1])));
  assert.deepEqual(sorted(scripts), ["about.js", "settings.js"]);
  for (const script of scripts) assert.ok(existsSync(join(root, "src", script)), `missing frontend script ${script}`);

  const resources = new Set(Object.values(html).flatMap((source) => resourcePaths(stripHtmlComments(source))));
  assert.deepEqual(sorted(resources), expectedResources);
  for (const resource of resources) assert.ok(existsSync(join(root, "src", resource)), `missing frontend resource ${resource}`);

  const faces = Object.entries(css).flatMap(([path, source]) => fontFaces(stripCssComments(source)).map((face) => ({ ...face, path })));
  const consumers = new Set(Object.values(css).flatMap((source) => [...usedFontFamilies(stripCssComments(source))]));
  assert.ok(consumers.has("Geist"), "Geist must remain used by shipped CSS");
  assert.ok(consumers.has("Phosphor"), "Phosphor must remain used by shipped CSS");
  for (const face of faces.filter((face) => face.family !== "Inter")) {
    const target = join(root, dirname(face.path), face.url);
    assert.ok(existsSync(target), `${face.path} references missing ${face.url}`);
  }
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
  assert.throws(() => validateIconCss(trimmedCss.replace(/\.ph\s*\{[^}]*\}/s, ""), usedIcons),
    /base .ph/, "the trimming validator must reject a missing base rule");
  assert.throws(() => validateIconCss(trimmedCss.replace(/@font-face\s*\{[^}]*\}/s, ""), usedIcons),
    /font face/, "the trimming validator must reject a missing font face");

  for (const [path, source] of Object.entries(js)) {
    const uncommented = stripComments(source);
    assert.deepEqual([...incompleteIconFragments(uncommented, new Set(expectedIcons))], [],
      `${path} dynamically assembles an icon class`);
  }
});

test("strips JavaScript comments without losing later frontend contracts", () => {
  let strippedCodeUnits = 0;
  let fullLineCommentTextCodeUnits = 0;
  for (const [path, source] of Object.entries({ "src/settings.js": settingsJs, "src/about.js": js["src/about.js"] })) {
    const uncommented = stripComments(source);
    const sourceLines = source.split("\n");
    const strippedLines = uncommented.split("\n");
    const fullLineComments = sourceLines.filter((line) => line.trimStart().startsWith("//"));
    assert.equal(uncommented.length, source.length, `${path} preserves character positions`);
    assert.equal(strippedLines.length, sourceLines.length, `${path} preserves line count`);
    for (let index = 0; index < source.length; index += 1) {
      if (source[index] === "\n") assert.equal(uncommented[index], "\n", `${path} preserves newline ${index}`);
    }
    assert.equal(strippedLines.filter((line) => line.trimStart().startsWith("//")).length, 0,
      `${path} retains no full-line comments`);
    assert.doesNotMatch(uncommented, /\/\*|\*\//, `${path} retains no block comment delimiters`);
    for (let index = 0; index < source.length; index += 1) {
      strippedCodeUnits += Number(source[index] !== "\n" && uncommented[index] !== source[index]);
    }
    fullLineCommentTextCodeUnits += fullLineComments.reduce(
      (total, line) => total + line.trimStart().replaceAll(/\s/g, "").length,
      0
    );
  }
  assert.equal(strippedCodeUnits, fullLineCommentTextCodeUnits,
    "strips exactly the non-whitespace full-line comment text after indentation");
  const uncommentedSettingsJs = stripComments(settingsJs);
  for (const line of [489, 586]) assert.equal(uncommentedSettingsJs.split("\n")[line - 1].trim(), "", `settings.js line ${line} is stripped`);
  for (const anchor of ["substring(0, 64)", "ph-x", "invoke("]) {
    assert.ok(uncommentedSettingsJs.includes(anchor), `settings.js loses ${anchor} after comment stripping`);
  }
  const cases = [
    ["const label = '// don\\'t'; // don't swallow\nconst retained = true;", "const retained = true;", "// don't swallow"],
    ["const quote = /[\"']/g; // comment\nconst retained = true;", "const quote = /[\"']/g;", "// comment"],
    ["const quotient = a / b; // c\nconst retained = true;", "const quotient = a / b;", "// c"],
    ["const slash = /[\\/]/;", "const slash = /[\\/]/;"],
    ['const url = "http://x.y";', 'const url = "http://x.y";'],
    ["/* it's */\nconst retained = true;", "const retained = true;", "/* it's */"],
    ["/* see dir/*.js */ const retained = true;", "const retained = true;", "/* see dir/*.js */"],
    ["const template = `// stays`;", "const template = `// stays`;"],
  ];
  for (const [source, retained, removed] of cases) {
    const stripped = stripComments(source);
    assert.ok(stripped.includes(retained), source);
    if (removed) assert.ok(!stripped.includes(removed), source);
  }
});

test("rejects dynamic icon assembly while accepting the shipped literal icons", () => {
  const allowedIcons = new Set(expectedIcons);
  const mutants = [
    '`<i class="ph ph-${name}"></i>`',
    "'<i class=\"ph ph-' + name + '\"></i>'",
    '"ph"+"-"+name',
    '["ph", name].join("-")',
    '`ph-${name}`',
    '"ph-" + name',
  ];
  for (const mutant of mutants) assert.ok(incompleteIconFragments(mutant, allowedIcons).size > 0, mutant);
  for (const literal of ["'ph-x'", '<i class="ph ph-gear">']) {
    assert.equal(incompleteIconFragments(literal, allowedIcons).size, 0, literal);
  }
  // HTML classes are static and pinned by expectedIcons; only JavaScript can assemble one dynamically.
  for (const [path, source] of Object.entries(js)) {
    assert.deepEqual([...incompleteIconFragments(stripComments(source), allowedIcons)], [], path);
  }
});
