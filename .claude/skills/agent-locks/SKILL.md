---
name: agent-locks
description: Claim files with the agent-locks MCP server before changing anything in this repository, and respect other agents' claims. Use before the first Edit, Write, or file-modifying shell command of any task (code, docs, scripts, packaging, third_party), when creating a worktree or branch for a change, and when finishing or abandoning work. Not needed for read-only work (reading, searching, answering questions).
---

# Agent locks

Several agents may work on this repository at once, in this checkout or in other worktrees of it.
The agent-locks MCP server (`mcp__agent-locks__*`) is how they see each other: a lock says "I am
changing these files, and here is how far I have got". Locks live in the repository's shared git
directory, so every worktree sees the same set, and they are never committed.

## Before the first change

1. `lock_query` (no arguments): see what is being worked on right now.
2. `lock_check_conflict` with globs for every path you expect to touch, e.g.
   `["crates/lindroids-cli/src/run.rs", "crates/lindroids-cli/src/flatpak/**"]`. Include files you
   will only touch a little (a new `mod` line in `main.rs`, a line in the README).
3. If an active lock overlaps:
   - Do not edit the overlapping files. Read the lock (title, tasks, notes) to see whether the
     overlap is real.
   - Narrow your own work to avoid the claimed files if you can, or work in a separate worktree
     on its own branch so the two changes meet only at merge time.
   - Otherwise stop and tell the user which lock blocks you (id, title, progress) and ask how to
     proceed. Never edit claimed files on your own judgement that the other agent "is probably
     done".
4. `lock_create` with a short title, the narrowest globs that cover your work, and the task list
   you are about to do (one task per real step: "implement X in run.rs", "update README",
   "cargo test"). Leave `agent_id` and `parent_agent_id` out unless your harness gave you one.

## While working

- `lock_update` each task the moment it is done, not in a batch at the end. Add a `note` when
  something others should know comes up (a changed interface, a file you had to add).
- If you need a file outside your scope, run `lock_check_conflict` for it, then `lock_create` a
  second, small lock for it. Do not quietly edit unclaimed files.
- Untracked or modified files you did not create belong to someone else's work in progress.
  Leave them alone even if no lock covers them.

## When done

- `lock_finish` with a one-line summary: what changed, where (branch or worktree, commit if
  any), and anything left undone.
- Finish the lock also when abandoning or handing back the work, with a summary that says so.
  An active lock nobody is working on blocks everyone else.

## Other agents' locks

Never finish, edit, or work around a lock you did not create. If one looks stale (no progress for
a long time, its work already merged), tell the user and let them decide.
