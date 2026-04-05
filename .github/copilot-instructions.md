# MANDATORY RULES
These rules are non-negotiable and override any other instruction.

1. **Always use `ask_user` after any action** — never end a turn with plain text questions. Every time you complete an action (fix, commit, feature, explanation, etc.), you MUST call `ask_user` to offer next steps.
2. **Always include a "done" option** — every `ask_user` call MUST include a choice that lets the user end the conversation. Write it in the same language the user is currently using (e.g., "Eso es todo por ahora" in Spanish, "That's all for now" in English, etc.).
3. **Never assume the conversation is over** — keep the feedback loop running until the user selects or writes a clear close signal (e.g., chooses the "done" option, or says something equivalent). Do not break the loop by ending with a statement.
4. **Choices must be meaningful** — offer 2–4 concrete next steps relevant to what was just done (e.g., "Test it in the browser", "Commit the changes", "Refactor X"). Do not offer vague options like "Do you want anything else?".
5. **Break down large or multi-topic messages** — BEFORE executing anything, apply this rule whenever the user sends a message that contains ANY of the following:
   - Multiple separate requests or tasks
   - A single request that describes more than one issue, symptom, or concern (e.g., "there are 3 problems with X: …")
   - A bug or feedback report with several sub-points, even if it reads as one paragraph
   - A list of things to fix, review, or implement, even if phrased as one sentence
   When any of these apply:
   a. Identify and list each topic/issue/sub-point as a numbered item.
   b. Use `ask_user` to confirm whether the breakdown is correct or needs adjustment.
   c. Work through one item at a time, confirming completion before moving to the next.
   **Do NOT treat "it sounds like one topic" as an exception. If there are multiple distinct concerns, list them.**
6. **Maintain a per-session conversation log** — at the start of any working session, create or update `.github/conversations/<SESSION_ID>.md` (where SESSION_ID is the Copilot session context ID). Track each identified topic with a status marker:
   - `[ ]` Pending
   - `[→]` In progress
   - `[x]` Resolved
   - `[+]` Added during the conversation
   Update the file as topics progress. If new topics are added mid-conversation, append them with `[+]` before changing to another status.

<!-- GSD Configuration — managed by get-shit-done installer -->
# Instructions for GSD

- Use the get-shit-done skill when the user asks for GSD or uses a `gsd-*` command.
- Treat `/gsd-...` or `gsd-...` as command invocations and load the matching file from `.github/skills/gsd-*`.
- When a command says to spawn a subagent, prefer a matching custom agent from `.github/agents`.
- Do not apply GSD workflows unless the user explicitly asks for them.
- After completing any `gsd-*` command (or any deliverable it triggers: feature, bug fix, tests, docs, etc.), ALWAYS: (1) offer the user the next step by prompting via `ask_user`; repeat this feedback loop until the user explicitly indicates they are done.
<!-- /GSD Configuration -->
