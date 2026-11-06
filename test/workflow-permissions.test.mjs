/**
 * @spec-handoff
 * @interface assertWorkflowPermissions(workflows: Map<string, object>): void
 * @behavior
 *   - Pins the effective permission map and declaration mode for every workflow job.
 *   - Requires CI and distribution jobs to inherit only their documented top-level grant.
 *   - Rejects unknown workflows and jobs, including jobs with extra read scopes.
 *   - Allows only Release Please to call the release workflow and release only by workflow_call.
 *   - Confines signing secrets to release and public-environment access to signing or cask jobs.
 * @edge-cases
 *   - Rejects read-all and write-all permission shorthands.
 *   - Reports the workflow path and job for every missing table entry.
 * @see ../.github/workflows/release-please.yml
 */

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { parseDocument } from "yaml";

const root = fileURLToPath(new URL("..", import.meta.url));
const workflowsDirectory = join(root, ".github/workflows");
const expectedTopLevelPermissions = new Map([
  ["ci.yml", { contents: "read" }],
  ["distribute.yml", { contents: "read" }],
  ["release-please.yml", {}],
  ["release.yml", { contents: "read" }],
]);
const expectedJobPermissions = new Map([
  ["ci.yml:commitlint", { declaration: "inherit", permissions: { contents: "read" } }],
  ["ci.yml:contract-tests", { declaration: "inherit", permissions: { contents: "read" } }],
  ["ci.yml:lint-and-check", { declaration: "inherit", permissions: { contents: "read" } }],
  ["ci.yml:cargo-audit", { declaration: "inherit", permissions: { contents: "read" } }],
  ["ci.yml:build", { declaration: "inherit", permissions: { contents: "read" } }],
  ["distribute.yml:update-homebrew", { declaration: "inherit", permissions: { contents: "read" } }],
  ["release-please.yml:release-please", {
    declaration: "explicit",
    permissions: { contents: "write", "pull-requests": "write", issues: "write" },
  }],
  ["release-please.yml:build", { declaration: "explicit", permissions: { contents: "write" } }],
  ["release-please.yml:distribute", { declaration: "explicit", permissions: { contents: "read" } }],
  ["release.yml:create-release", { declaration: "explicit", permissions: { contents: "write" } }],
  ["release.yml:build-macos", { declaration: "explicit", permissions: { contents: "write" } }],
  ["release.yml:build-linux", { declaration: "explicit", permissions: { contents: "write" } }],
  ["release.yml:report-linux", { declaration: "explicit", permissions: {} }],
]);

const readWorkflows = () => new Map(readdirSync(workflowsDirectory)
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
  .sort()
  .map((name) => {
    const path = join(workflowsDirectory, name);
    const document = parseDocument(readFileSync(path, "utf8"));
    assert.deepEqual(document.errors, [], `.github/workflows/${name} must parse without errors`);
    return [name, document.toJS()];
  }));

const label = (workflow, job) => `.github/workflows/${workflow} job ${job}`;
const triggerNames = (workflow) => {
  const triggers = Object.hasOwn(workflow, "on") ? workflow.on : workflow.true;
  if (typeof triggers === "string") return [triggers];
  if (Array.isArray(triggers)) return triggers;
  return triggers && typeof triggers === "object" ? Object.keys(triggers) : [];
};
const hasPublicEnvironment = (job) => job.environment === "public"
  || job.environment?.name === "public";
const expectedPublicEnvironmentJobs = new Set([
  "release.yml:build-macos",
  "distribute.yml:update-homebrew",
]);
const allowedTapReferences = new Set(["github.token", "secrets.TAP_GITHUB_TOKEN"]);
const assertWorkflowPermissions = (workflows) => {
  assert.deepEqual(
    [...workflows.keys()],
    [...expectedTopLevelPermissions.keys()],
    "workflow file set",
  );

  for (const [name, workflow] of workflows) {
    const workflowPath = `.github/workflows/${name}`;
    assert.ok(
      Object.hasOwn(workflow, "permissions"),
      `${workflowPath} must declare top-level permissions`,
    );
    assert.deepEqual(
      workflow.permissions,
      expectedTopLevelPermissions.get(name),
      `${workflowPath} top-level permissions`,
    );
    assert.ok(
      workflow.jobs && typeof workflow.jobs === "object",
      `${workflowPath} must declare jobs`,
    );

    for (const [jobName, job] of Object.entries(workflow.jobs)) {
      const jobLabel = label(name, jobName);
      const hasOwnPermissions = Object.hasOwn(job, "permissions");
      const expected = expectedJobPermissions.get(`${name}:${jobName}`);
      assert.ok(
        expected,
        `${jobLabel} is not in the exact permission table`,
      );
      if (expected.declaration === "inherit") {
        assert.equal(hasOwnPermissions, false, `${jobLabel} must inherit top-level permissions`);
        assert.deepEqual(
          workflow.permissions,
          expected.permissions,
          `${jobLabel} inherited permissions`,
        );
        continue;
      }
      assert.equal(hasOwnPermissions, true, `${jobLabel} must declare permissions`);
      assert.notEqual(job.permissions, "read-all", `${jobLabel} must not use read-all`);
      assert.notEqual(job.permissions, "write-all", `${jobLabel} must not use write-all`);
      assert.deepEqual(job.permissions, expected.permissions, `${jobLabel} permissions`);
    }
  }
};

test("requires least-privilege permissions for every workflow and job", () => {
  assertWorkflowPermissions(readWorkflows());
});

test("allows release builds only from the release-please reusable-workflow caller", () => {
  const workflows = readWorkflows();
  const release = workflows.get("release.yml");

  assert.deepEqual(
    triggerNames(release).sort(),
    ["workflow_call"],
    ".github/workflows/release.yml triggers",
  );

  const callers = [...workflows].flatMap(([workflowName, workflow]) => Object.entries(workflow.jobs)
    .filter(([, job]) => job.uses === "./.github/workflows/release.yml")
    .map(([jobName]) => `${workflowName}:${jobName}`));
  assert.deepEqual(callers, ["release-please.yml:build"], "release workflow callers");
});

test("confines signing secrets and the public environment to release and cask jobs", () => {
  const workflows = readWorkflows();
  const release = workflows.get("release.yml");
  const signingNames = [...new Set(
    JSON.stringify(release).match(/(?:APPLE_[A-Z_]+|TAURI_SIGNING_PRIVATE_KEY(?:_PASSWORD)?)/g) ?? [],
  )];
  assert.ok(signingNames.length > 0, "release workflow must name its signing secrets");

  for (const [workflowName, workflow] of workflows) {
    for (const [jobName, job] of Object.entries(workflow.jobs)) {
      const jobLabel = label(workflowName, jobName);
      assert.equal(
        hasPublicEnvironment(job),
        expectedPublicEnvironmentJobs.has(`${workflowName}:${jobName}`),
        `${jobLabel} public environment access`,
      );
      if (workflowName !== "release.yml") {
        for (const signingName of signingNames) {
          assert.equal(
            JSON.stringify(job).includes(signingName),
            false,
            `${jobLabel} must not reference ${signingName}`,
          );
        }
      }
      if (workflowName === "distribute.yml" && jobName === "update-homebrew") {
        const references = JSON.stringify(job).match(/(?:secrets\.[A-Z0-9_]+|github\.token)/g) ?? [];
        assert.ok(
          references.every((reference) => allowedTapReferences.has(reference)),
          `${jobLabel} may use only its tap secret and github.token`,
        );
      }
    }
  }
});
