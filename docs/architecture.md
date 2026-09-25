# fael architecture

> **Status:** the log format and storage (§2, [format.md](format.md)) are implemented in `fael-core`; the CLI, MCP,
> hooks and maintenance commands are design. This page is the contract the code is built against —
> when code and this page disagree, fix one of them in the same commit.

fael is a memory log for coding agents that lives **inside the repo**: every agent (Claude Code, Codex, OpenCode, …)
and every person on the team reads and writes the same log, git carries it between machines, and there is no server.

Three ideas carry the whole design:

1. **The log is the truth; everything else is derived** — like Redis's append-only file or a Kafka topic.
2. **Agents are made to write, not asked** — hooks refuse to end a turn that committed work without a memory row.
3. **Memory comes to the agent** — when an agent reads a file, the rows about that file are attached to the read.

---

## 1. Parts

```
┌──────────────────────── fael (one binary) ────────────────────────┐
│                                                                    │
│   adapters            core                         log             │
│   ─────────           ────                         ───             │
│   CLI         ──┐     validate · append ──────▶   .fael/log/**    │
│   MCP (stdio) ──┼──▶  find · rank · budget ◀────  (jsonl, in git) │
│   hook        ──┘     decide (block / context)                     │
│    ├ claude                                                        │
│    ├ codex                                                         │
│    ├ opencode                                                      │
│    └ neutral  ← public protocol for any other agent                │
└────────────────────────────────────────────────────────────────────┘
```

| Part | Job | Knows about |
|---|---|---|
| **log** | stores rows — the only source of truth | nothing (plain files) |
| **core** | validates, appends, finds, ranks, decides | the row format · never a client |
| **adapters** | turn each client's input/output into core calls | one client each · never the rules |

Adding a client touches only an adapter. Changing a rule touches only core. Changing the format is a spec change.

## 2. Storage

```
.fael/
  config.toml                  optional — every field has a default
  log/
    <writer>/
      2026-09.jsonl            rows written this month — append-only
      2026-09.close.jsonl      closes written this month — append-only
      compact.<ULID>.jsonl     immutable, from `fael compact`
    _import/<ULID>.jsonl       immutable, from `fael import`
  .lock                        not in git — serialises local writers
```

- `<writer>` = `<git user.name slug>-<4 hex of sha256(email)>` — the same person on two machines shares a folder, and two people who share a name do not.
- One file per month: last month's file is never written again, so rotation needs no command.
- `.gitattributes`: `.fael/log/**/*.jsonl merge=union` — two branches that both appended keep both sides; readers remove the duplicates by `id`.

**Row** (one JSON object per line — full spec in [format.md](format.md)):

```json
{"v":1,"id":"01J8ZQ3K4M7N2P5R8T1V4X6Y9A","ts":"2026-09-25T10:00:00Z","by":"delamind-3f9a",
 "kind":"decision","text":"…","files":["src/a.rs"],"key":"auth:session"}
```

- `id` is a ULID — time-sortable, and it never collides across machines.
- `kind` is `decision`, `issue` or `note`, plus any kinds the repo declares in `config.toml`.
- `files` must name at least one file (or anchor such as `doc:pricing`). This is enforced.
- `key` uses Redis-style names (`auth:session:timeout`) and is queried with glob patterns (`auth:*`).
- A close is its own row, written to the `.close.jsonl` file. It never edits the row it closes.

**Write safety:**
- A line is committed once its trailing `\n` is written.
- Local writers are serialised by `File::lock` on `.fael/.lock`.
- Files are rewritten only with tmp-then-rename, and only when no one writes to them any more.
- Durability comes from git; rows are not fsynced one by one.

## 3. API

### CLI

| Command | What it does |
|---|---|
| `fael add <kind> "<text>" --files a,b [--key k] [--supersedes id]` | append a row |
| `fael close <id> "<why>"` | append a close row |
| `fael find [text] [--files …] [--key glob] [--kind …] [--since …] [--all] [--branches]` | query; closed and superseded rows are hidden unless `--all` |
| `fael keys [glob]` | list keys, with a count and last use for each — check here before you invent a new key |
| `fael kickoff [anchor]` | the session brief: open issues, recent decisions, rows on other branches |
| `fael hook <event> [--client c]` | hook entry point (see below) |
| `fael mcp` | MCP server on stdio |
| `fael install [--dry-run]` | detect installed clients and wire MCP, hooks and skill into each one |
| `fael compact` · `fael import <path> [--map old/=new/]` | maintenance |
| `fael doctor [--fix]` | find and repair damaged logs — `--fix` moves bad lines to quarantine, it never deletes them |
| `fael stats` | how many bytes and tokens fael has put into agents' context |

### MCP (3 tools — each schema is paid for in every session, so the list stays short)

| Tool | Input | Notes |
|---|---|---|
| `find` | `files[]` `text` `key` `kind` `since` `limit` | read-only |
| `add` | `kind` `text` `files[]` (required, non-empty) `key?` `supersedes?` | a bad value is rejected with an error message that says how to fix the call |
| `close` | `id` `text` | |

### Hook protocol

Each client speaks its own hook format. The binary contains the adapters for the supported clients; everyone else uses the neutral format.

```
fael hook <stop|session-start|read|edit> [--client claude|codex|opencode]   < stdin  > stdout

neutral Event  {"event","cwd","session","client","files":[…],"stop_active"}
neutral Reply  {"block":bool,"reason"?:str,"context"?:str}
```

The hook always exits 0. If fael hits an internal error it replies with an empty Reply, because a memory tool must never break the agent's tool call.

## 4. Data flow

**Write** — an agent records something:
```
agent ─(MCP add | CLI add)─▶ core.validate ─✗─▶ error that says how to fix the call
                                  │✓
                                  ▼
                   lock ─▶ append one line ─▶ unlock      (.fael/log/<writer>/<month>.jsonl)
```

**Push** — the agent reads a file, and the memory for that file comes with it:
```
client ─(read event)─▶ adapter.parse ─▶ core.find(files) ─▶ rank ─▶ cut to token budget ─▶ adapter.render ─▶ context attached to the read
```
Ranking: an exact file match beats the same directory, which beats the same key. Open `issue` and `decision` rows go first, and older rows are weighted down.

**Enforce** — the agent tries to end a turn:
```
client ─(stop event)─▶ core: commits this turn? ─no─▶ allow
                                 │yes
                                 ▼
                         new row this turn? ─yes─▶ allow
                                 │no
                                 ▼
                         block once, with the reason and the exact command to run
```

**Session start:**
```
client ─(session-start)─▶ core.kickoff ─▶ open issues · recent decisions · N rows on unmerged branches ─▶ context
```

**Across branches** (one branch per person or per agent):
```
git for-each-ref ─▶ git cat-file --batch  .fael/log/** on each ref ─▶ dedupe by id ─▶ find / kickoff
```
Reading needs no checkout. Rows are written only to your own branch and reach `main` through the normal pull request.

## 5. Tokens

Tokens are the unit of value, and they are spent when reading, not when storing. So:

- Storage is JSON, so any tool can parse it and it merges cleanly in git.
- What an agent is shown (kickoff, push, `find`) is one markdown line per row: `- [id] kind #key text → files`. On real rows this adds 18.5% on top of the text, against 42.6% for raw JSON and 17.7% for TOON. The text is most of the size, so the savings come from choosing fewer rows, not from the format.
- Every output is cut to a token budget (configurable per repo). The estimate is computed at read time and never stored, because every model's tokenizer counts differently. The estimator is calibrated in tests against real tokenizers, and its error is published.
- Every injection is recorded per machine (`~/.local/state/fael/usage.jsonl`, never in git), so `fael stats` shows fael's real cost in context.
- A row longer than about 400 estimated tokens triggers a warning when it is written, because it is paid for every time it is pushed. The hard limit is 10 KiB per row.

## 6. Self-healing

Reading never fails: broken lines, leftover merge-conflict markers, duplicate ids, CRLF and BOM are all handled in memory. Writing seals a torn last line before it appends. `fael doctor` reports problems, and `--fix` repairs them with tmp-then-rename. Bad lines go to `.fael/quarantine/`, so no byte is ever deleted.

## 7. Non-goals

A query language, a daemon or server, embeddings, and hand-written tags or links. Links come for free from shared `files`, shared `key` and `supersedes`.
