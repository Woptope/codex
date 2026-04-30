# Continue Button Companion

Small browser companion for Codex App Server sessions that this companion creates,
resumes, and keeps in its local session list.

## Run

Start an app-server websocket listener:

```bash
codex app-server --listen ws://127.0.0.1:8765
```

Build and serve the companion:

```bash
node scripts/serve.mjs
```

Open `http://127.0.0.1:4173`, connect to the websocket URL, then start or
resume sessions from the companion UI.

## Behavior

- The session list is populated only from sessions started or resumed in this
  browser companion.
- `Continue` is disabled until a session is selected.
- Idle sessions receive one `turn/start` request with the exact text
  `continue`.
- Stopped sessions are resumed before receiving `continue`.
- Running sessions receive `turn/interrupt`; the companion waits for the active
  turn to reach a terminal state before sending `continue`.
- Repeated clicks are single-flighted, so one click burst sends at most one
  prompt.

## Tests

```bash
node --experimental-strip-types --test test/*.test.ts
```

## Smoke Check

1. Start `codex app-server --listen ws://127.0.0.1:8765`.
2. Start the companion with `node scripts/serve.mjs`.
3. Create or resume two sessions in the companion.
4. Select one session and click `Continue`.
5. Verify only the selected session receives the `continue` turn.

This example does not modify `/Applications/Codex.app`, `~/.codex`, or the
installed desktop app update path.
