# Automatic Fresh-Session Handoff

Codex can optionally start a fresh session after repeated live context
compactions. This is useful for long-running TUI workflows where compaction keeps
the thread alive, but a replacement session would give the model a cleaner
working context.

## Configuration

```toml
# Start a fresh session after this many live context compactions.
# Unset or set to 0 to disable.
auto_new_session_after_compactions = 2

# Keep a root session active while it is coordinating live thread-spawn
# subagents. Child subagents can still hand off to replacement sessions.
auto_new_session_after_compactions_children_only_with_subagents = true
```

The setting is opt-in. If `auto_new_session_after_compactions` is unset or `0`,
Codex only compacts context and does not create replacement sessions.

## Handoff Behavior

When the configured compaction threshold is reached, Codex asks the current
thread to prepare a handoff prompt, starts a replacement thread, and submits that
prompt as the replacement thread's first user input. The TUI follows the
replacement root thread automatically. For thread-spawn subagents, the TUI marks
the old child thread closed, adds the replacement child thread, and selects it.

Internal sessions do not trigger automatic handoff. They continue to rely on
normal context compaction.

## Live Subagents

By default, root threads replace themselves after the configured number of live
compactions. When
`auto_new_session_after_compactions_children_only_with_subagents = true`, a root
thread with live thread-spawn subagents keeps running instead of interrupting and
replacing itself. This avoids disrupting the parent session while it is
coordinating live child agents.

With the children-only setting enabled:

- A root thread with no live subagents still hands off normally.
- A root thread with live thread-spawn subagents suppresses its own replacement.
- Thread-spawn subagents can still hand off to replacement child threads.
- Replacement child threads retain their parent linkage, depth, nickname, agent
  path, role, and environment selections.

## Goal Preservation

Replacement root threads and replacement thread-spawn subagent threads copy the
old thread's active persisted goal snapshot. Active and budget-limited goals keep
their goal id, objective, status, token budget, token usage, time usage, and
timestamps in the replacement thread.

Paused, completed, or otherwise inactive goals are not copied. If a root thread
suppresses replacement because it has live subagents, the parent thread and its
goal are left unchanged.
