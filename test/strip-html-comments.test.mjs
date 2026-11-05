/**
 * @spec-handoff
 * @interface stripHtmlComments(source: string): string
 * @behavior
 *   - Removes HTML comments without leaving an opening comment delimiter.
 *   - Preserves markup outside comments, including adjacent comments.
 * @edge-cases
 *   - A malformed or unterminated comment is removed through the end of input.
 *   - Delimiters introduced by removing malformed nested input are removed too.
 * @see ./helpers/strip-html-comments.mjs
 */

import assert from "node:assert/strict";
import test from "node:test";

import { stripHtmlComments } from "./helpers/strip-html-comments.mjs";

test("strips malformed, adjacent, and unterminated HTML comments without removing markup", () => {
  const cases = [
    ["<!<!-- x -->-->", ""],
    ["<main><!-- a --><!-- b --><p>kept</p></main>", "<main><p>kept</p></main>"],
    ["<main>kept</main><!-- x", "<main>kept</main>"],
    ["<main><p>kept</p></main>", "<main><p>kept</p></main>"],
  ];

  for (const [source, expected] of cases) {
    const actual = stripHtmlComments(source);
    assert.equal(actual, expected);
    assert.equal(actual.includes("<!--"), false);
  }
});
