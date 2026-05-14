# Sample configuration

For a sample configuration file, see [this documentation](https://developers.openai.com/codex/config-sample).

## Automatic fresh-session handoff

```toml
# Start a fresh session after two live context compactions.
# Unset or set to 0 to disable.
auto_new_session_after_compactions = 2

# When the root session has live thread-spawn subagents, keep the root active
# and allow only child subagents to hand off to replacement sessions.
auto_new_session_after_compactions_children_only_with_subagents = true
```
