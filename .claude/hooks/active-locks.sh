#!/bin/sh
# SessionStart: tell a new session which agent-locks are active (shared by every worktree).
dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)/agents-locks"
set -- "$dir"/*.md
[ -e "$1" ] || { echo "agent-locks: no active locks."; exit 0; }
echo "agent-locks: active locks (claimed files; see the agent-locks skill before editing):"
for f in "$@"; do
	title=$(sed -n 's/^# //p' "$f" | head -n 1)
	scope=$(sed -n '/^scope:/,/^---/s/^  - //p' "$f" | paste -sd ' ' -)
	done_=$(grep -c '^- \[x\]' "$f"); all=$(grep -c '^- \[[ x]\]' "$f")
	echo "- $(basename "$f" .md): $title [$done_/$all] $scope"
done
