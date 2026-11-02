/**
 * @spec-handoff
 * @interface releaseMetadata(): { packageVersion: string, tauriVersion: string }
 * @behavior
 *   - Keeps package.json and src-tauri/tauri.conf.json on one release version.
 *   - Requires CI and release Linux jobs to invoke one repository-owned installer.
 *   - Keeps macOS and Linux tagged-release artifact jobs wired to tauri-action.
 * @edge-cases
 *   - Missing installer or workflow wiring fails with an actionable assertion.
 * @see ../.github/workflows/release.yml
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const read = (path) => readFileSync(new URL(path, `file://${root}/`), "utf8");

test("keeps the frontend and Tauri release versions synchronized", () => {
  const packageVersion = JSON.parse(read("package.json")).version;
  const tauriVersion = JSON.parse(read("src-tauri/tauri.conf.json")).version;

  assert.equal(packageVersion, tauriVersion);
});

test("keeps the required Linux packages in one repository-owned installer", () => {
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
});

test("makes Linux CI and tagged releases invoke the shared prerequisite installer", () => {
  const ci = read(".github/workflows/ci.yml");
  const release = read(".github/workflows/release.yml");

  for (const workflow of [ci, release]) {
    assert.match(workflow, /scripts\/install-linux-dependencies\.sh/);
    assert.doesNotMatch(workflow, /apt-get install/);
  }
});

test("keeps tagged macOS and Linux release jobs on the pinned Tauri action", () => {
  const release = read(".github/workflows/release.yml");
  const macosJob = release.slice(
    release.indexOf("  build-macos:"),
    release.indexOf("  build-linux:"),
  );
  const linuxJob = release.slice(release.indexOf("  build-linux:"));

  for (const job of [macosJob, linuxJob]) {
    assert.match(job, /uses: tauri-apps\/tauri-action@[0-9a-f]{40} # v[\d.]+/);
    assert.match(job, /tagName: \$\{\{ inputs\.tag_name \}\}/);
  }
});
