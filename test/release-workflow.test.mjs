/**
 * @spec-handoff
 * @interface releaseWorkflowContracts(): void
 * @behavior
 *   - Keeps release metadata, release-please paths, and Linux packaging configuration aligned.
 *   - Requires the release workflow to create one release and build both platforms in parallel.
 *   - Requires the Linux failure report to read the action outcome rather than job result.
 *   - Keeps CI builds independent of commitlint and pins CI/release Linux runners to Ubuntu 24.04.
 * @edge-cases
 *   - Release tags flow through environment variables, not shell-expanded workflow expressions.
 *   - Linux build failure remains observable despite job-level continue-on-error.
 * @see ../.github/workflows/release.yml
 */

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const read = (path) => readFileSync(new URL(path, `file://${root}/`), "utf8");

const jobBlock = (workflow, name) => {
  const header = new RegExp(`^  ${name}:\\s*$`, "m");
  const match = header.exec(workflow);
  assert.notEqual(match, null, `missing ${name} job`);
  const start = match.index;
  const next = workflow.slice(start + match[0].length).search(/^  [\w-]+:/m);
  return withoutYamlComments(workflow.slice(start, next === -1 ? undefined : start + match[0].length + next));
};

const runBodies = (job) => [...job.matchAll(/^ {8}run:[ \t]*(.*)$/gm)].map((match) => {
  const value = match[1];
  if (!/^[>|][+-]?[ \t]*(?:#.*)?$/.test(value)) return value;

  const lines = job.slice(match.index + match[0].length).split(/(?<=\n)/);
  const body = [];
  for (const line of lines) {
    if (!/^[ \t]*\n$/.test(line) && !/^ {10,}/.test(line)) break;
    body.push(line);
  }
  return body.join("");
});

const needsJobs = (job) => {
  const match = /^ {4}needs:[ \t]*(.*)$/m.exec(job);
  if (!match) return [];
  if (match[1]) {
    return match[1].replace(/\s+#.*$/, "").trim().replace(/^\[|\]$/g, "")
      .split(",").map((name) => name.trim());
  }

  return job.slice(match.index + match[0].length)
    .match(/^ {6}- ([\w-]+)$/gm)?.map((line) => line.trim().slice(2)) ?? [];
};

const matrixOs = (job) => {
  const match = /^ {8}os:[ \t]*(.*)$/m.exec(job);
  if (!match) return [];
  if (match[1]) return match[1].replace(/^\[|\]$/g, "").split(",").map((label) => label.trim());

  return job.slice(match.index + match[0].length)
    .match(/^ {10}- ([\w.-]+)$/gm)?.map((line) => line.trim().slice(2)) ?? [];
};

// CI and release artifact builds use Ubuntu 24.04; macOS remains macos-latest for its Apple Silicon DMG.
const runnerLabels = (job) => {
  const match = /^ {4}runs-on:[ \t]*(.*)$/m.exec(job);
  assert.notEqual(match, null, "job must declare runs-on");
  const label = match[1].trim();
  return label === "${{ matrix.os }}" ? matrixOs(job) : [label];
};

// This small YAML scanner does not distinguish block scalar contents from YAML comments.
const withoutYamlComments = (yaml) => yaml.split("\n").map((line) => {
  if (/^\s*#/.test(line)) return "";
  let quote = "";
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (quote) {
      if (character === quote && (quote === "'" || line[index - 1] !== "\\")) quote = "";
    } else if (character === "'" || character === "\"") {
      quote = character;
    } else if (character === "#" && /\s/.test(line[index - 1] ?? "")) {
      return line.slice(0, index);
    }
  }
  return line;
}).join("\n");

test("keeps the frontend and Tauri release versions synchronized", () => {
  const packageVersion = JSON.parse(read("package.json")).version;
  const tauriVersion = JSON.parse(read("src-tauri/tauri.conf.json")).version;
  const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(read("src-tauri/Cargo.toml"))?.[1];

  assert.equal(packageVersion, tauriVersion);
  assert.equal(packageVersion, cargoVersion);
});

test("keeps release-please extra files inside the package path and synchronized", () => {
  const extraFiles = JSON.parse(read("release-please-config.json")).packages["src-tauri"]["extra-files"];
  const packageFile = extraFiles.find((file) => file.path === "/package.json");
  const tauriFile = extraFiles.find((file) => file.path === "tauri.conf.json");

  assert.deepEqual(packageFile, { type: "json", path: "/package.json", jsonpath: "$.version" });
  assert.deepEqual(tauriFile, { type: "json", path: "tauri.conf.json", jsonpath: "$.version" });
  for (const file of extraFiles) assert.doesNotMatch(file.path, /\.\./, "release-please paths cannot escape src-tauri");
});

test("keeps Linux packaging limited to deb without updater artifacts", () => {
  const linux = JSON.parse(read("src-tauri/tauri.linux.conf.json"));

  assert.deepEqual(linux.bundle.targets, ["deb"]);
  assert.equal(linux.bundle.createUpdaterArtifacts, false);
});

test("keeps the required Linux packages and executable installer in one repository-owned script", (t) => {
  const installer = read("scripts/install-linux-dependencies.sh");
  const installerPath = new URL("scripts/install-linux-dependencies.sh", `file://${root}/`);

  for (const packageName of [
    "build-essential",
    "libgtk-3-dev",
    "libwebkit2gtk-4.1-dev",
    "libayatana-appindicator3-dev",
    "librsvg2-dev",
    "patchelf",
  ]) {
    assert.match(installer, new RegExp(`\\b${packageName}\\b`));
  }
  // The development packages pull pkgconf; rustls does not require OpenSSL headers.
  assert.doesNotMatch(installer, /\b(?:pkg-config|libssl-dev)\b/);
  assert.doesNotMatch(installer, /--no-install-recommends/);
  assert.notEqual(statSync(installerPath).mode & 0o111, 0, "installer must be executable on disk");

  try {
    const entry = execFileSync("git", ["ls-files", "-s", "scripts/install-linux-dependencies.sh"], {
      cwd: root,
      encoding: "utf8",
    });
    assert.match(entry, /^100755\s/);
  } catch (error) {
    const details = `${error?.stderr ?? ""}${error?.message ?? ""}`;
    if (/pathspec .* did not match any files/.test(details)) throw error;
    t.skip("cannot inspect tracked executable mode");
  }
});

test("uses sudo only when the Linux dependency installer is not root", () => {
  const installer = read("scripts/install-linux-dependencies.sh");

  assert.match(installer, /SUDO=\(\)\s*\nif \[ "\$\(id -u\)" -ne 0 \]; then\s*\n  SUDO=\(sudo\)\s*\nfi/);
  for (const line of installer.split("\n").filter((line) => /\bapt-get\b/.test(line))) {
    assert.match(line, /^"\$\{SUDO\[@\]\}"\s+apt-get\b/);
  }
});

test("makes Linux CI and tagged releases invoke the shared prerequisite installer", () => {
  const ci = read(".github/workflows/ci.yml");
  const release = read(".github/workflows/release.yml");

  for (const workflow of [ci, release]) {
    assert.match(workflow, /scripts\/install-linux-dependencies\.sh/);
    assert.doesNotMatch(workflow, /apt-get install/);
  }
});

test("keeps CI builds independent from commitlint while retaining commitlint as a separate check", () => {
  const ci = read(".github/workflows/ci.yml");
  const build = jobBlock(ci, "build");

  jobBlock(ci, "commitlint");
  assert.deepEqual(needsJobs(jobBlock(ci, "lint-and-check")), []);
  assert.deepEqual(needsJobs(jobBlock(ci, "cargo-audit")), []);
  assert.deepEqual(needsJobs(build).sort(), ["cargo-audit", "lint-and-check"]);
  assert.deepEqual(needsJobs(jobBlock(ci, "contract-tests")), []);
});

test("runs Node contract tests independently in pull request CI", () => {
  const contractTests = jobBlock(read(".github/workflows/ci.yml"), "contract-tests");
  const runs = runBodies(contractTests);

  assert.match(contractTests, /^ {4}runs-on: ubuntu-24\.04$/m);
  assert.match(contractTests, /actions\/checkout@[0-9a-f]{40} # v[\d.]+/);
  assert.match(contractTests, /actions\/setup-node@[0-9a-f]{40} # v[\d.]+/);
  assert.match(contractTests, /node-version: 26/);
  assert.equal(runs.indexOf("npm ci") < runs.indexOf("npm test"), true,
    "npm ci must run before npm test");
});

test("parses flow-style needs with trailing whitespace and comments", () => {
  assert.deepEqual(
    needsJobs("  build:\n    needs: [lint-and-check, cargo-audit]  # direct prerequisites\n"),
    ["lint-and-check", "cargo-audit"],
  );
});

test("resolves the expected number of runner labels in CI and release workflows", () => {
  for (const [path, expectedCount] of [
    [".github/workflows/ci.yml", 6],
    [".github/workflows/release.yml", 4],
  ]) {
    const workflow = read(path);
    const jobs = [...workflow.slice(workflow.indexOf("\njobs:\n")).matchAll(/^ {2}([\w-]+):\s*$/gm)]
      .map((match) => match[1]);
    const labels = jobs.flatMap((name) => runnerLabels(jobBlock(workflow, name)));

    assert.equal(labels.length, expectedCount, `${path} resolved runner-label count`);
  }
});

test("pins every Linux CI and release runner, including matrix Linux, to ubuntu-24.04", () => {
  for (const path of [".github/workflows/ci.yml", ".github/workflows/release.yml"]) {
    const workflow = read(path);
    const jobs = [...workflow.slice(workflow.indexOf("\njobs:\n")).matchAll(/^ {2}([\w-]+):\s*$/gm)]
      .map((match) => match[1]);
    for (const name of jobs) {
      for (const label of runnerLabels(jobBlock(workflow, name))) {
        if (label.startsWith("ubuntu-")) assert.equal(label, "ubuntu-24.04", `${path} ${name}`);
        if (label.startsWith("macos-")) assert.equal(label, "macos-latest", `${path} ${name}`);
      }
    }
    assert.doesNotMatch(workflow, /ubuntu-latest/, `${path} must not use ubuntu-latest`);
  }
});

test("pins every external workflow action to a SHA with a version comment", () => {
  for (const [path, expectedExternalActions] of [
    [".github/workflows/release.yml", 10],
    [".github/workflows/ci.yml", 13],
    [".github/workflows/distribute.yml", 1],
    [".github/workflows/release-please.yml", 1],
  ]) {
    const externalActions = read(path).split("\n")
      .map((line) => /^\s*(?:-\s+)?uses:\s*(.+)$/.exec(line)?.[1])
      .filter((action) => action && !action.startsWith("./.github/workflows/"));

    assert.equal(externalActions.length, expectedExternalActions, `${path} external action count`);
    for (const action of externalActions) {
      assert.match(action, /^[\w-]+\/[\w.-]+@[0-9a-f]{40} # (v[\d.]+|action-v[\d.]+)$/);
    }
  }
});

test("creates or finds one release through an environment-provided tag and exposes its ID", () => {
  const createRelease = jobBlock(read(".github/workflows/release.yml"), "create-release");
  const runs = runBodies(createRelease);

  assert.match(createRelease, /contents: write/);
  assert.match(createRelease, /release-id: \$\{\{ steps\.release\.outputs\.release-id \}\}/);
  assert.match(createRelease, /TAG_NAME: \$\{\{ inputs\.tag_name \}\}/);
  assert.equal(runs.length, 1);
  assert.doesNotMatch(runs[0], /\$\{\{\s*inputs\.tag_name\s*\}\}/);
  assert.doesNotMatch(createRelease, /gh api --method PATCH|gh release edit/);
});

test("builds macOS and Linux in parallel after release creation", () => {
  const release = read(".github/workflows/release.yml");
  const macos = jobBlock(release, "build-macos");
  const linux = jobBlock(release, "build-linux");

  for (const job of [macos, linux]) {
    assert.deepEqual(needsJobs(job), ["create-release"]);
  }
});

test("passes the created release ID and selected tag to both release builds", () => {
  const release = read(".github/workflows/release.yml");

  for (const job of [jobBlock(release, "build-macos"), jobBlock(release, "build-linux")]) {
    assert.match(job, /releaseId: \$\{\{ needs\.create-release\.outputs\.release-id \}\}/);
    assert.match(job, /tagName: \$\{\{ inputs\.tag_name \}\}/);
  }
});

test("keeps updater publishing and Apple signing exclusive to the public macOS build", () => {
  const release = read(".github/workflows/release.yml");
  const macos = jobBlock(release, "build-macos");
  const linux = jobBlock(release, "build-linux");

  assert.match(macos, /^ {4}environment: public$/m);
  assert.match(macos, /uploadUpdaterJson: true/);
  assert.match(macos, /APPLE_/);
  assert.match(macos, /TAURI_SIGNING_PRIVATE_KEY/);
  assert.match(linux, /uploadUpdaterJson: false/);
  assert.doesNotMatch(linux, /APPLE_|TAURI_SIGNING_PRIVATE_KEY/);
});

test("preserves the Linux action outcome when its nonblocking build fails", () => {
  const linux = jobBlock(read(".github/workflows/release.yml"), "build-linux");

  assert.match(linux, /^ {4}continue-on-error: true$/m);
  assert.match(linux, /id: tauri\n {8}uses: tauri-apps\/tauri-action@[0-9a-f]{40}/);
  assert.match(linux, /linux-build-failed: \$\{\{ steps\.tauri\.outcome != 'success' \}\}/);
});

test("reports Linux action failure without using a masked job result", () => {
  const report = jobBlock(read(".github/workflows/release.yml"), "report-linux");
  const runs = runBodies(report);

  assert.match(report, /^ {4}if: always\(\)$/m);
  assert.match(report, /^ {4}needs: \[build-linux\]$/m);
  assert.match(report, /^ {4}permissions: \{\}$/m);
  assert.match(report, /needs\.build-linux\.outputs\.linux-build-failed/);
  assert.doesNotMatch(report, /needs\.build-linux\.result/);
  assert.equal(runs.length, 1);
  assert.doesNotMatch(runs[0], /\$\{\{/);
});

test("keeps the release build and distribution workflow chain intact", () => {
  const releasePlease = read(".github/workflows/release-please.yml");
  const build = jobBlock(releasePlease, "build");
  const distribute = jobBlock(releasePlease, "distribute");

  assert.match(build, /uses: \.\/\.github\/workflows\/release\.yml/);
  assert.match(distribute, /^ {4}needs: \[release-please, build\]$/m);
  assert.match(distribute, /uses: \.\/\.github\/workflows\/distribute\.yml/);
  assert.match(read(".github/workflows/distribute.yml"), /^jobs:/m);
});
