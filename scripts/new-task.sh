#!/usr/bin/env bash
# New task: a worktree in ../work/ferrobot-<name> on branch <kind>/<name>, from an up-to-date origin/main.
# Usage: scripts/new-task.sh <name> [feat|fix|perf|refactor|docs|chore]
set -euo pipefail
name="${1:?usage: new-task.sh <name> [kind]}"
kind="${2:-feat}"
case "$kind" in feat|fix|perf|refactor|docs|chore) ;; *) echo "unknown kind: $kind" >&2; exit 1;; esac
case "$name" in *[!a-z0-9-]*|"") echo "name: lower case letters, digits and dashes" >&2; exit 1;; esac
repo="$(cd "$(dirname "$0")/.." && pwd)"
dir="$(dirname "$repo")/work/ferrobot-$name"
branch="$kind/$name"
[ -e "$dir" ] && { echo "already exists: $dir" >&2; exit 1; }
git -C "$repo" fetch -q origin
mkdir -p "$(dirname "$dir")"
git -C "$repo" worktree add -q -b "$branch" "$dir" origin/main
git -C "$dir" branch -q --unset-upstream 2>/dev/null || true
mkdir -p "$dir/.scratch"
echo "worktree: $dir"
echo "branch:   $branch"
