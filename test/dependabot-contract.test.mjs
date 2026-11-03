/**
 * @spec-handoff
 * @interface assertDependabotPolicy(config: object): void
 * @behavior
 *   - Requires exactly three bounded weekly version-update groups.
 *   - Keeps Dependabot security updates separate and the zbus major exclusion Cargo-only.
 * @edge-cases
 *   - Rejects major updates, cooldowns, missing applies-to, and changed limits.
 * @see ../.github/dependabot.yml
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parseDocument } from "yaml";

const expectedUpdates = {
  "github-actions": {
    directory: "/",
    labels: ["dependencies", "ci"],
    prefix: "ci",
  },
  npm: {
    directory: "/",
    labels: ["dependencies", "javascript"],
    prefix: "build",
  },
  cargo: {
    directory: "/src-tauri",
    labels: ["dependencies", "rust"],
    prefix: "build",
  },
};
const expectedGroup = {
  "applies-to": "version-updates",
  patterns: ["*"],
  "update-types": ["minor", "patch"],
};
const expectedCargoIgnore = [{
  "dependency-name": "zbus",
  "update-types": ["version-update:semver-major"],
}];

const knownGoodConfig = () => ({
  version: 2,
  updates: Object.entries(expectedUpdates).map(([ecosystem, expected]) => ({
    "package-ecosystem": ecosystem,
    directory: expected.directory,
    schedule: { interval: "weekly", day: "monday" },
    "open-pull-requests-limit": 3,
    "commit-message": { prefix: expected.prefix, include: "scope" },
    labels: expected.labels,
    groups: { "all-dependencies": structuredClone(expectedGroup) },
    ...(ecosystem === "cargo" ? { ignore: structuredClone(expectedCargoIgnore) } : {}),
  })),
});

const assertDependabotPolicy = (config) => {
  assert.equal(config.version, 2);
  assert.equal(config.updates.length, 3);
  const updates = new Map(config.updates.map((update) => [update["package-ecosystem"], update]));
  assert.deepEqual([...updates.keys()].sort(), Object.keys(expectedUpdates).sort());

  for (const [ecosystem, expected] of Object.entries(expectedUpdates)) {
    const update = updates.get(ecosystem);
    assert.equal(update.directory, expected.directory, `${ecosystem} directory`);
    assert.deepEqual(update.labels, expected.labels, `${ecosystem} labels`);
    assert.deepEqual(update.schedule, { interval: "weekly", day: "monday" }, `${ecosystem} schedule`);
    assert.equal(update["open-pull-requests-limit"], 3, `${ecosystem} limit`);
    assert.deepEqual(update["commit-message"], { prefix: expected.prefix, include: "scope" }, `${ecosystem} commit message`);
    assert.deepEqual(
      update.groups,
      { "all-dependencies": expectedGroup },
      `${ecosystem} groups`,
    );
    for (const forbidden of ["time", "timezone"]) assert.equal(update.schedule[forbidden], undefined, `${ecosystem} schedule.${forbidden}`);
    for (const forbidden of ["cooldown", "dependency-type"]) assert.equal(update[forbidden], undefined, `${ecosystem} must not set ${forbidden}`);
    if (ecosystem !== "cargo") assert.equal(update.ignore, undefined, `${ecosystem} must not have ignore`);
  }

  assert.deepEqual(updates.get("cargo").ignore, expectedCargoIgnore, "cargo zbus ignore");
};

test("keeps Dependabot version updates grouped and bounded", () => {
  const document = parseDocument(readFileSync(new URL("../.github/dependabot.yml", import.meta.url), "utf8"));

  assert.deepEqual(document.errors, [], "dependabot.yml must parse without errors");
  assertDependabotPolicy(document.toJS());
});

test("rejects weakened Dependabot grouping policy", () => {
  const mutations = [
    (config) => delete config.updates[0].groups,
    (config) => config.updates[0].groups["all-dependencies"]["update-types"].push("major"),
    (config) => config.updates[1].groups["all-dependencies"]["update-types"].splice(0, 1),
    (config) => config.updates[2].ignore[0]["update-types"] = ["version-update:semver-minor"],
    (config) => config.updates[0].cooldown = { "default-days": 7 },
    (config) => config.updates[1]["open-pull-requests-limit"] = 1,
    (config) => delete config.updates[2].groups["all-dependencies"]["applies-to"],
    (config) => config.updates[0].groups.security = {
      "applies-to": "security-updates",
      patterns: ["*"],
    },
    (config) => config.updates[1].groups["all-dependencies"].unexpected = true,
    (config) => config.updates[0]["commit-message"].prefix = "ci(deps)",
    (config) => config.updates[2]["commit-message"].prefix = "build(deps)",
  ];

  assert.doesNotThrow(() => assertDependabotPolicy(knownGoodConfig()));
  for (const mutate of mutations) {
    const mutant = knownGoodConfig();
    mutate(mutant);
    assert.throws(() => assertDependabotPolicy(mutant));
  }
});
