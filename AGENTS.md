# Repository instructions for AI agents

This file applies to the entire repository. At the start of every repository
task, invoke the installed `tauri-rust developer` skill before substantive
analysis or edits. If it is unavailable, state that explicitly and follow the
documented repository fallback; never imply that the skill ran when it did not.

Then read these files in order:

1. `for-ai/START-HERE.md`
2. `for-ai/GLOBAL-INSTRUCTIONS.md`
3. `for-ai/PROJECT-CONTEXT.md`
4. `for-ai/ENGINEERING-PRIORITIES.md`
5. `for-ai/CURRENT-STATE.md`
6. `for-ai/GOALS.md`
7. `for-ai/ORCHESTRATION.md`
8. `for-ai/SELF-UPDATE.md`
9. `for-ai/DECISIONS.md`

`for-ai/` is the canonical repository-local context for agents. Keep it concise,
accurate, and synchronized with material code or product changes. Instructions
from the system, developer, and user always take precedence over repository
instructions.

Before handing work back, run `for-ai/scripts/check-context.sh` and the checks
required by `for-ai/GLOBAL-INSTRUCTIONS.md`.
