#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo_root"

printf '%s\n' '== repository =='
git remote get-url origin 2>/dev/null || printf '%s\n' 'origin unavailable'
printf '%s\n' '== branch and worktree =='
git status --short --branch
printf '%s\n' '== recent commits =='
git log -5 --oneline --decorate
printf '%s\n' '== declared checks =='
grep -A 12 '^## Required validation' for-ai/GLOBAL-INSTRUCTIONS.md
