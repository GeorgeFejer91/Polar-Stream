#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
required_files="
AGENTS.md
for-ai/README.md
for-ai/START-HERE.md
for-ai/GLOBAL-INSTRUCTIONS.md
for-ai/PROJECT-CONTEXT.md
for-ai/ENGINEERING-PRIORITIES.md
for-ai/CURRENT-STATE.md
for-ai/GOALS.md
for-ai/ORCHESTRATION.md
for-ai/SELF-UPDATE.md
for-ai/DECISIONS.md
for-ai/templates/HANDOFF.md
"

missing=0
for relative_path in $required_files; do
  if [ ! -s "$repo_root/$relative_path" ]; then
    printf 'missing or empty: %s\n' "$relative_path" >&2
    missing=1
  fi
done

for relative_path in START-HERE.md GLOBAL-INSTRUCTIONS.md PROJECT-CONTEXT.md ENGINEERING-PRIORITIES.md CURRENT-STATE.md GOALS.md ORCHESTRATION.md SELF-UPDATE.md DECISIONS.md; do
  if ! grep -Fq "for-ai/$relative_path" "$repo_root/AGENTS.md"; then
    printf 'AGENTS.md does not route to for-ai/%s\n' "$relative_path" >&2
    missing=1
  fi
done

if ! grep -Fq 'tauri-rust developer' "$repo_root/for-ai/START-HERE.md"; then
  printf 'START-HERE.md does not require the tauri-rust developer skill\n' >&2
  missing=1
fi

if grep -R -n -E '(gho_[A-Za-z0-9]+|BEGIN (RSA |OPENSSH )?PRIVATE KEY|api[_-]?key[[:space:]]*[:=][[:space:]]*[^[:space:]]+)' "$repo_root/for-ai" >/dev/null 2>&1; then
  printf 'possible credential material detected in for-ai/\n' >&2
  missing=1
fi

if [ "$missing" -ne 0 ]; then
  exit 1
fi

printf 'for-ai context is present and internally routed\n'
