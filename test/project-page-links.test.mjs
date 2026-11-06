/**
 * @spec-handoff
 * @interface assertProjectPageLinks(): void
 * @behavior
 *   - Requires the generated Homebrew cask to emit the public project page as its homepage.
 *   - Requires README.md to expose the public project page through an inline Markdown link.
 *   - Rejects conflicting Website, Homepage, or Project page links in project documentation.
 * @edge-cases
 *   - Links inside fenced code blocks do not count as documentation links.
 *   - The cask homepage must not fall back to the GitHub repository URL.
 * @see ../.github/workflows/distribute.yml
 */

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const projectPage = "https://shoootyou.dev/projects/stay-awake/";
const withoutFencedCode = (markdown) => {
  let inFence = false;
  return markdown.split("\n").filter((line) => {
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      return false;
    }
    return !inFence;
  }).join("\n");
};
const markdownFiles = (directory) => readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
  const path = join(directory, entry.name);
  if (entry.isDirectory()) return markdownFiles(path);
  return entry.isFile() && entry.name.endsWith(".md") ? [path] : [];
});

test("writes the public project page as the generated cask homepage", () => {
  const workflow = readFileSync(join(root, ".github/workflows/distribute.yml"), "utf8");
  const generatorStart = workflow.indexOf("cat > Casks/stay-awake.rb << EOF");
  const generatorEnd = workflow.indexOf("\n          EOF", generatorStart);

  assert.notEqual(generatorStart, -1, "cask generator heredoc");
  assert.notEqual(generatorEnd, -1, "cask generator heredoc terminator");
  const cask = workflow.slice(generatorStart, generatorEnd);
  assert.ok(cask.includes(`homepage "${projectPage}"`), "generated cask homepage");
  assert.doesNotMatch(cask, /homepage "https:\/\/github\.com\/\$\{REPO\}"/, "legacy cask homepage");
});

test("links the README to the public project page outside code fences", () => {
  const readme = withoutFencedCode(readFileSync(join(root, "README.md"), "utf8"));

  assert.match(
    readme,
    /\[[^\]\n]+\]\(https:\/\/shoootyou\.dev\/projects\/stay-awake\/\)/,
    "README project-page link",
  );
});

test("uses the public project page for every labeled documentation homepage link", () => {
  const documentation = [
    ...readdirSync(root, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith(".md"))
      .map((entry) => join(root, entry.name)),
    ...markdownFiles(join(root, "docs")),
  ];
  const homepageLinks = documentation.flatMap((path) => {
    const markdown = withoutFencedCode(readFileSync(path, "utf8"));
    return [...markdown.matchAll(/\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)/g)]
      .filter(([, label]) => /\b(?:website|homepage|project page)\b/i.test(label))
      .map(([, , url]) => ({ file: relative(root, path), url }));
  });

  assert.deepEqual(homepageLinks, [{ file: "README.md", url: projectPage }]);
});
