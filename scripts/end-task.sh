#!/usr/bin/env bash
# After the PR is merged: delete the task's worktree and its local branch. Run from the main checkout.
# Usage: scripts/end-task.sh <name>
set -euo pipefail
name="${1:?usage: end-task.sh <name>}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
dir="$(dirname "$repo")/work/ferrobot-$name"
[ -d "$dir" ] || { echo "no worktree: $dir" >&2; exit 1; }
case "$PWD/" in "$dir"/*) echo "leave $dir first" >&2; exit 1;; esac
branch="$(git -C "$dir" branch --show-current)"
if [ -n "$(git -C "$dir" status --porcelain --untracked-files=no)" ]; then
    echo "$dir has uncommitted changes; not deleted" >&2
    exit 1
fi
git -C "$repo" worktree remove --force "$dir"
git -C "$repo" branch -D "$branch" >/dev/null && echo "deleted $dir and branch $branch"
