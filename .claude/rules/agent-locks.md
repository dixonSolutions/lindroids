# Agent locks

Other agents may be changing this repository at the same time, here or in another worktree.

- Before the first file change of any task, follow the `agent-locks` skill: `lock_query`,
  `lock_check_conflict` with the globs you will touch, then `lock_create` with your task list.
- Never edit files an active lock claims. Narrow your work, use a separate worktree, or ask.
- `lock_update` each task as it completes; `lock_finish` with a summary when done or abandoned.
- Modified or untracked files you did not create are someone else's work: leave them alone and
  never stage them in your commits.
- Read-only work (reading, searching, answering) needs no lock.
