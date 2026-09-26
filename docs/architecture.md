# fael architecture

> **Status:** the log format and storage (§2, [format.md](format.md)) are implemented in `fael-core`, and so are
> `add` `close` `find` `keys` `kickoff` `mv` in the `fael` CLI (`find --branches` not yet), `fael mcp`
> (stdio, 3 tools), `fael hook <stop|session-start|read|edit>` (neutral + claude/codex adapters) with
> per-machine usage accounting (`fael stats`), `fael install` (Claude Code, Codex, OpenCode),
> and the maintenance commands `fael doctor [--fix]` · `fael compact` · `fael import` (SPEC §6, §11).
> This page is the contract the code is built against —
> when code and this page disagree, fix one of them in the same commit.

fael is a memory log for agents that lives **inside the repo**: every agent (Claude Code, Codex, OpenCode, a chat
host speaking MCP, …) and every person on the team reads and writes the same log, git carries it between machines, and there is no server.

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
│   CLI         ──┐     normalize·validate·append ▶ .fael/log/**    │
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
| **core** | validates, appends, finds, ranks, decides — `add_row` · `close_row` · `query` · `Config::from_toml` | the row format · never a client, never git or cwd: the adapter passes `Stamp { by, branch, sha }` and the config text |
| **adapters** | turn each client's input/output into core calls | one client each · never the rules — paths go through `core.normalize`, never an adapter's own cleanup |

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

- `<writer>` = `<git user.name slug>-<4 hex of sha256(email)>` — a writer is a **logical author, not a machine**: the same person on two machines shares a folder (their appends meet in git via union merge), and two people who share a name do not.
- One file per month: last month's file is never written again, so rotation needs no command.
- `.gitattributes`: `.fael/log/**/*.jsonl merge=union` — two branches that both appended keep both sides; readers remove the duplicates by `id`.

**Row** (one JSON object per line — full spec in [format.md](format.md)):

```json
{"v":1,"id":"01J8ZQ3K4M7N2P5R8T1V4X6Y9A","ts":"2026-09-25T10:00:00Z","by":"delamind-3f9a",
 "kind":"decision","text":"…","files":["src/a.rs"],"key":"auth:session"}
```

- `id` is a ULID — time-sortable, and it never collides across machines.
- `kind` is `decision`, `issue` or `note`, plus any kinds the repo declares in `config.toml`.
- `files` is fael's **locality index**: stable references the row is about — it is what makes memory come to the agent, and why fael needs no tags, links or graph. At least one is enforced, in one of two forms:
  - **path** `src/auth/session.ts` — normalised to repo-relative
  - **anchor** `scheme:ref` (`doc:pricing`, `issue:#12`, `customer:acme`) — for memory with no file behind it. fael checks only the syntax; the ref is opaque (`/` in `doc:pricing/2026` is not a path) and there is no registry of schemes — what a scheme means belongs to the agent.
- `key` is optional — a row with only `files` is complete. When used it is a stable namespace with Redis-style names (`auth:session:timeout`) and is queried with glob patterns (`auth:*`).
- A close is its own row, written to the `.close.jsonl` file. It never edits the row it closes.

**Write safety:**
- A line is committed once its trailing `\n` is written.
- Local writers are serialised by `File::lock` on `.fael/.lock`.
- Files are rewritten only with tmp-then-rename, and only when no one writes to them any more.
- Durability comes from git; rows are not fsynced one by one.

## 3. API

### Integration modes

The storage and core are identical; only how the agent reaches fael differs.

- **Enforcement** — coding agents with lifecycle hooks: `agent → hook → fael`. Session brief, memory attached to reads, stop enforcement.
- **Tool** — chat agents and MCP hosts without hooks: `agent → MCP → fael`. The agent calls `find` at the start and `add` on its own for durable things (an explicit "remember", a rule, a stable preference, a correction) — never for chatter. That judgement is agent behaviour (skill / tool description), not a core rule. Tool mode has no commit step: rows reach git only when the host or the user commits `.fael/`.

A standard-compliant MCP host needs no adapter — `fael mcp` is the whole integration.

### CLI

| Command | What it does |
|---|---|
| `fael add <kind> "<text>" --files a,b [--key k] [--to who] [--supersedes id]` | append a row |
| `fael close <id> "<why>"` | append a close row |
| `fael find [text] [--files …] [--key glob] [--kind …] [--since …] [--to who] [--all] [--branches]` | query; closed and superseded rows are hidden unless `--all` |
| `fael keys [glob]` | list keys, with a count and last use for each — to reuse a key that already exists |
| `fael mv <old> <new>` | record a move git can't see — an anchor, an uncommitted rewrite, or one file split into several (one old path may point at many new ones). Adds matches only, never hides a row |
| `fael kickoff [anchor]` | the session brief: open issues, then the rest by freshness (newer of the row and its files' last change); rows whose files are all gone are left out |
| `fael hook <event> [--client c]` | hook entry point (see below) |
| `fael mcp` | MCP server on stdio |
| `fael install [--client c] [--dry-run] [--replace-fapony]` | detect installed clients and wire MCP, hooks and skill into each one; `--replace-fapony` takes out fapony's Stop/session-start hooks and MCP (opt-in: they are user scope and still serve repos without `.fael/`) |
| `fael compact` · `fael import <path> [--map old/=new/]` | maintenance |
| `fael doctor [--fix]` | find and repair damaged logs — `--fix` moves bad lines to quarantine, it never deletes them |
| `fael stats` | how many bytes and tokens fael has put into agents' context |

`--files` in `find` matches a row's `files[]` only — exactly, as a directory (a zone), or by glob; an anchor's ref
never matches as a directory. It does not fall back to searching text (fapony did); text is `find <text>`.
Queries expand through rename aliases first, so a row filed under a path that was renamed since (`git log -M`,
cached in `.fael/cache/aliases.json`, plus `fael mv` rows) still matches at the new path.
Ids are accepted as a unique prefix and printed at the shortest length that stays unique (≥ 8).

`.fael/config.toml` — every field is optional:

```toml
kinds = ["risk"]              # extra kinds on top of decision/issue/note
key_domains = ["auth", "db"]  # first key segment; outside the list = warning, never a reject
resolve = true                # follow renames (git log -M + fael mv rows); false = match files[] literally
[budget]
kickoff_tokens = 800          # kickoff, and find with no filter
find_tokens = 800
push_tokens = 800             # read/edit hook push
session_decisions = 0         # session-start lists this many freshest open decisions above the count line
[warn]
row_tokens = 400
[limit]
row_bytes = 10240             # hard cap, never above 10 KiB
```

### MCP (3 tools on stdio — each schema is paid for in every session, so the list stays short)

| Tool | Input | Notes |
|---|---|---|
| `find` | `files[]` `text` `key` `kind` `since` `to` `limit` | read-only, cut to `budget.find_tokens`. No filter = the session brief (what `kickoff` shows) — so there is no `kickoff` tool |
| `add` | `kind` `text` `files[]` (required, non-empty) `key?` `to?` `supersedes?` | a bad value is rejected with an error message that says how to fix the call. Its description tells the agent to reuse an anchor `find` already showed rather than invent a new one |
| `close` | `id` `text` | |

### Hook protocol

Each client speaks its own hook format. The binary contains the adapters for the supported clients; everyone else uses the neutral format.

```
fael hook <stop|session-start|read|edit> [--client claude|codex]   < stdin  > stdout

neutral Event  {"cwd","session","client","files":[…],"stop_active","text"}
neutral Reply  {"block":bool,"reason"?:str,"context"?:str}
```

OpenCode has no Stop hook and runs plugins in-process: `fael install` writes a JS plugin that speaks the neutral format (a stop block becomes a prompt on `session.idle`). How to wire any other agent: [integrate.md](integrate.md).

The hook always exits 0. If fael hits an internal error it replies with an empty Reply, because a memory tool must never break the agent's tool call.

## 4. Data flow

**Write** — an agent records something:
```
agent ─(MCP add | CLI add | hook)─▶ core.normalize ─▶ core.validate ─✗─▶ error that says how to fix the call
                                                          │✓
                                                          ▼
                                       lock ─▶ append one line ─▶ unlock      (.fael/log/<writer>/<month>.jsonl)
```

**Push** — the agent reads a file, and the memory for that file comes with it:
```
client ─(read event)─▶ adapter.parse ─▶ core.find(files) ─▶ rank ─▶ cut to token budget ─▶ adapter.render ─▶ context attached to the read
```
Ranking: an exact file match beats the same directory, which beats the same key. Open `issue` and `decision` rows go first, then newer before older by `id`. Text matching is plain substring.
Ranking is **deterministic**: the same log, query and budget give the same output on any machine and any day — recency comes from `id` order, never from the clock, and ties break by `id`. No fuzzy, BM25 or semantic ranking.

**Enforce** — the agent tries to end a turn:
```
client ─(edit event)─▶ append {"path","at"} to ~/.local/state/fael/sessions/<session+worktree>.jsonl

client ─(stop event)─▶ edits recorded this session?
                          │yes                                │no
                          ▼                                   ▼
                any edit after the session's        git commits since start
                newest row (or any, if none)?       and no row this session?
                          │                                   │
                   no ─▶ allow  yes ─┐            no ─▶ allow  yes ─┐
                                     ▼                              ▼
                        block once per last row — a markdown list of the files
                        (or commits) and the exact command, --files prefilled
```
Edits, not commits, are the primary signal: many agents are told never to commit, and a commit-only rule never fires for them. Git is only the fallback for edits the hook never saw (a shell `sed`, a heredoc). Measuring from the newest row, not the session start, keeps a row filed early from covering hours of work after it; a new row reopens one more block. The edit list is per-machine runtime state, never in `.fael/`. The `session` string sent with `edit` must equal the one sent with `stop`.

**Session start:**
```
client ─(session-start)─▶ open issues to you in full · N freshest open decisions (opt-in) · count line for the rest ─▶ context
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
- Every injection is recorded per machine (`~/.local/state/fael/usage.jsonl`, never in git), so `fael stats` shows fael's real cost in context. This is **local telemetry, not memory**: it may be lost, is never read to answer a query, and a failure to write it never fails a command. `.fael/` is the only semantic state.
- A row longer than about 400 estimated tokens triggers a warning when it is written, because it is paid for every time it is pushed. The hard limit is 10 KiB per row.

## 6. Self-healing

Reading never fails: broken lines, leftover merge-conflict markers, duplicate ids, CRLF and BOM are all handled in memory. Writing seals a torn last line before it appends. `fael doctor` reports problems, and `--fix` repairs them with tmp-then-rename. Bad lines go to `.fael/quarantine/`, so no byte is ever deleted.

## 7. Non-goals

A query language, a daemon, embeddings, and hand-written tags or links. Links come for free from shared `files`, shared `key` and `supersedes`.

The local tool never needs a daemon or a server. A hosted server (MCP over HTTP for web chat hosts) is a separate product built on `fael-core` and this same format — it is not part of this binary. Three rules bind it:
- **A repo, when there is one, is the truth** — the hosted side is a git client that commits rows into it; it is canonical storage only for workspaces with no repo. Syncing is a union of lines deduped by `id` (append-only + ULID), so there is nothing to resolve.
- **It calls the same core entry points as the CLI** — `Stamp.by` from the signed-in user, no branch/sha. Hook session state (`~/.local/state/fael`) is CLI-only: HTTP clients have no edit or stop events, so there is nothing to carry to a phone.
- **Export and import go through this public format without loss of semantic memory** — no data stays locked in the hosted side.
