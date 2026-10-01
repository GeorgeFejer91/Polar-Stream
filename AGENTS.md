# Repository instructions for AI agents

This file applies to the entire repository. At the start of every repository
task, invoke the installed `tauri-rust-developer` skill before substantive
analysis or edits. If it is unavailable, state that explicitly and follow the
documented repository fallback; never imply that the skill ran when it did not.

Then read these files in order:

1. `for-ai/START-HERE.md`
2. `for-ai/SKILLS.md`
3. `for-ai/GLOBAL-INSTRUCTIONS.md`
4. `for-ai/PROJECT-CONTEXT.md`
5. `for-ai/ENGINEERING-PRIORITIES.md`
6. `for-ai/CURRENT-STATE.md`
7. `for-ai/GOALS.md`
8. `for-ai/ORCHESTRATION.md`
9. `for-ai/SELF-UPDATE.md`
10. `for-ai/DECISIONS.md`

Then select the app contract named by the request. Read every contract only
when the change genuinely spans those products:

- `for-ai/apps/MULTI-STREAMER.md`
- `for-ai/apps/POLAR-STREAM-MINI.md`
- `for-ai/apps/VERNIER-STREAM-MINI.md`

`for-ai/` is the canonical repository-local context for agents. Keep it concise,
accurate, and synchronized with material code or product changes. Instructions
from the system, developer, and user always take precedence over repository
instructions.

Before handing work back, run `for-ai/scripts/check-context.sh`, the applicable
checks in `for-ai/GLOBAL-INSTRUCTIONS.md`, and the selected app contract's
package/install gate. The live GitHub Pages URL is required only for work whose
selected contract includes the main multi-streamer or download surface.
