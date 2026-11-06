/**
 * @spec-handoff
 * @interface assertWorkflowPermissions(workflows: Map<string, object>): void
 * @behavior
 *   - Requires every workflow and reusable-workflow caller to declare permissions.
 *   - Limits write grants to the documented release and release-please jobs.
 *   - Pins each workflow's top-level and caller-job permissions to least privilege.
 * @edge-cases
 *   - Rejects read-all and write-all permission shorthands.
 *   - Reports the workflow path and job for every job-level violation.
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
  ["release-please.yml:release-please", {
    contents: "write", "pull-requests": "write", issues: "write",
  }],
  ["release-please.yml:build", { contents: "write" }],
  ["release-please.yml:distribute", { contents: "read" }],
  ["release.yml:create-release", { contents: "write" }],
  ["release.yml:build-macos", { contents: "write" }],
  ["release.yml:build-linux", { contents: "write" }],
  ["release.yml:report-linux", {}],
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
const isContentsRead = (permissions) => permissions.contents === "read"
  && Object.keys(permissions).length === 1;
const isReusableCaller = (job) => typeof job.uses === "string"
  && job.uses.startsWith("./.github/workflows/");

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
      const inheritsLeastPrivilege = isContentsRead(workflow.permissions)
        || Object.keys(workflow.permissions).length === 0;
      assert.ok(
        hasOwnPermissions || inheritsLeastPrivilege,
        `${jobLabel} must declare permissions or inherit an empty/read-only top level`,
      );
      if (isReusableCaller(job)) {
        assert.ok(hasOwnPermissions, `${jobLabel} reusable caller must declare permissions`);
      }

      if (hasOwnPermissions) {
        assert.notEqual(job.permissions, "read-all", `${jobLabel} must not use read-all`);
        assert.notEqual(job.permissions, "write-all", `${jobLabel} must not use write-all`);
        assert.ok(
          job.permissions && typeof job.permissions === "object",
          `${jobLabel} permissions must be a mapping`,
        );
        for (const [scope, access] of Object.entries(job.permissions)) {
          if (access !== "write") continue;
          const expected = expectedJobPermissions.get(`${name}:${jobName}`);
          assert.equal(expected?.[scope], "write", `${jobLabel} must not grant ${scope}: write`);
        }
      }

      const expected = expectedJobPermissions.get(`${name}:${jobName}`);
      if (expected) assert.deepEqual(job.permissions, expected, `${jobLabel} permissions`);
    }
  }
};

test("requires least-privilege permissions for every workflow and job", () => {
  assertWorkflowPermissions(readWorkflows());
});
