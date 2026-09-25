# Integrate fael into an agent

fael needs four calls from an agent's lifecycle. Each is one process spawn:
Event JSON on stdin, Reply JSON on stdout, always exit 0 (about 2–3 ms each; `session-start` about 10 ms
because it runs one `git check-ignore`). Claude Code and Codex use a built-in adapter
(`--client claude|codex`), and OpenCode gets a generated plugin
([`fael/skill/opencode.js`](../fael/skill/opencode.js), the reference implementation of
this page). Any other agent calls the neutral format:

```
fael hook <stop|session-start|read|edit>   < Event   > Reply

Event  {"cwd": str, "session": str, "client": str, "files": [str], "stop_active": bool, "text": str}
Reply  {"block": bool, "reason"?: str, "context"?: str}
```

| When | Call | Send | Do with the Reply |
|---|---|---|---|
| session starts | `session-start` | `cwd`, `session` | put `context` in the system prompt / first turn |
| a file was read | `read` | `cwd`, `files` | append `context` to the tool result |
| a file was written | `edit` | `cwd`, `session`, `files` | append `context` to the tool result |
| the agent is about to end its turn | `stop` | `cwd`, `session`, `text`, `stop_active` | `block` → do not end; send `reason` back to the model as the next message |

- `session` — an RFC 3339 time the session started (`2026-09-25T10:00:00.000Z`), or a transcript
  file whose birthtime is the start. **Send the exact same string to `edit` and `stop`**: it keys the
  session's edit list, and stop blocks when files were edited after the newest row.
- `files` — absolute, or relative to `cwd`. Paths outside the repo are dropped.
- `text` — the assistant's text in this session (or at least the last message). The issue rule
  looks for "found a bug", "inconsistent", "might break", and similar; leave it out and fael reads `session` as a Claude transcript.
- `stop_active` — `true` when this stop comes right after one you blocked, so it never loops.
  fael also blocks each problem only once per session.
- `client` — a name for `fael stats`.

Any error, missing field or repo without `.fael/` is `{"block": false}`. Integration cannot break the agent.

Also register `fael mcp` (stdio, tools `find` / `add` / `close`). If the agent can load skills,
use the one `fael install` writes (`~/.claude/skills/fael/SKILL.md`).
