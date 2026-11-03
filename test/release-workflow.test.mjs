/**
 * @spec-handoff
 * @interface releaseMetadata(): { packageVersion: string, tauriVersion: string }
 * @behavior
 *   - Keeps release metadata, shared Linux prerequisites, and SHA-pinned actions aligned.
 *   - Requires the release workflow to create one release and build both platforms in parallel.
 *   - Requires the Linux failure report to read the action outcome rather than job result.
 * @edge-cases
 *   - Release tags flow through environment variables, not shell-expanded workflow expressions.
 *   - Linux build failure remains observable despite job-level continue-on-error.
 * @see ../.github/workflows/release.yml
 */

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const read = (path) => readFileSync(new URL(path, `file://${root}/`), "utf8");

const jobBlock = (workflow, name) => {
  const header = `  ${name}:`;
  const start = workflow.indexOf(header);
  assert.notEqual(start, -1, `missing ${name} job`);
  const next = workflow.slice(start + header.length).search(/^  [\w-]+:/m);
  return workflow.slice(start, next === -1 ? undefined : start + header.length + next);
};

const runBodies = (job) => [...job.matchAll(/^ {8}run: \|\n((?: {10}.*\n?)*)/gm)].map(([, body]) => body);

test("keeps the frontend and Tauri release versions synchronized", () => {
  const packageVersion = JSON.parse(read("package.json")).version;
  const tauriVersion = JSON.parse(read("src-tauri/tauri.conf.json")).version;

  assert.equal(packageVersion, tauriVersion);
});

test("keeps the required Linux packages and executable installer in one repository-owned script", (t) => {
  const installer = read("scripts/install-linux-dependencies.sh");

  for (const packageName of [
    "libgtk-3-dev",
    "libwebkit2gtk-4.1-dev",
    "libayatana-appindicator3-dev",
    "librsvg2-dev",
    "patchelf",
  ]) {
    assert.match(installer, new RegExp(`\\b${packageName}\\b`));
  }
  assert.doesNotMatch(installer, /--no-install-recommends/);

  try {
    const entry = execFileSync("git", ["ls-files", "-s", "scripts/install-linux-dependencies.sh"], {
      cwd: root,
      encoding: "utf8",
    });
    assert.match(entry, /^100755\s/);
  } catch (error) {
    if (error?.code === "ENOENT") t.skip("git is unavailable");
    else throw error;
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

test("pins every external workflow action to a SHA with a version comment", () => {
  for (const path of [
    ".github/workflows/release.yml",
    ".github/workflows/ci.yml",
    ".github/workflows/distribute.yml",
    ".github/workflows/release-please.yml",
  ]) {
    for (const line of read(path).split("\n").filter((line) => line.includes("uses:"))) {
      const action = line.slice(line.indexOf("uses:") + "uses:".length).trim();
      if (!action.startsWith("./.github/workflows/")) {
        assert.match(action, /^[\w-]+\/[\w.-]+@[0-9a-f]{40} # (v[\d.]+|action-v[\d.]+)$/);
      }
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
    assert.match(job, /^ {4}needs: create-release$/m);
  }
  assert.doesNotMatch(macos, /^ {4}needs: .*build-linux/m);
  assert.doesNotMatch(linux, /^ {4}needs: .*build-macos/m);
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
