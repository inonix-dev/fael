# fael log format v1

One page, public. Any tool that follows it can read and write a fael log without fael.
Reference implementation: [`fael-core`](../fael-core/src).

## Layout

```
.fael/
  config.toml                  optional
  log/
    <writer>/
      2026-09.jsonl            add rows written in that month (UTC, from ts) — append-only
      2026-09.close.jsonl      close rows written in that month — append-only
      compact.<ULID>.jsonl     immutable
    _import/<ULID>.jsonl       immutable
  .lock                        not in git
```

- `.gitattributes`: `.fael/log/**/*.jsonl merge=union`
- `<writer>` = `<slug of git user.name>-<first 4 hex of sha256(lowercase git user.email)>`, e.g. `delamind-3f9a`.
  Slug = lowercase ASCII letters and digits, anything else collapses to one `-`; empty → `anon`. No email → hash the hostname.
  The email is never written — only the hash. A writer id never starts with `_` or `.`.
- Files are UTF-8, LF, one JSON object per line.

## Rows

**Add row** — in `<writer>/<yyyy-mm>.jsonl`:

```json
{"v":1,"id":"01J8ZQ3K4M7N2P5R8T1V4X6Y9A","ts":"2026-09-25T10:00:00Z","by":"delamind-3f9a","kind":"decision","text":"…","files":["src/a.rs"],"key":"auth:session"}
```

| field | required | rule |
|---|---|---|
| `v` | yes | `1` |
| `id` | yes | writers emit a [ULID](https://github.com/ulid/spec); readers accept any unique string |
| `ts` | yes | RFC 3339 UTC. For humans — order comes from `id`, never `ts` |
| `by` | yes | writer id |
| `kind` | yes | `decision` · `issue` · `note`, or a kind listed in `config.toml` `kinds = [...]` |
| `text` | yes | non-empty, written to stand alone |
| `files` | yes, ≥ 1 | repo paths, or `scheme:ref` anchors (`issue:#12`, `doc:pricing`) |
| `key` | no | `:`-separated segments of `[a-z0-9._-]+`, ≤ 64 chars, e.g. `auth:session:timeout` |
| `supersedes` | no | id of an older row this one replaces |
| `client` `model` `session` `branch` `sha` | no | filled in by tools, never by the agent |

**Close row** — in `<writer>/<yyyy-mm>.close.jsonl`. Any kind can be closed; a close never edits the row.

```json
{"v":1,"id":"01J9…","ts":"…","by":"delamind-3f9a","ref":"01J8ZQ…","text":"fixed in 4139925"}
```

**Compact files** carry the close inside the row instead: `"closed":{"id","ts","by","text"}`.

## Writers

A writer must reject a row before writing it when: `files` is empty · `kind` is not allowed ·
`key` breaks the pattern · the serialised line is over 10 KiB (bytes) · it looks like a secret.

To append:
1. take an exclusive lock on `.fael/.lock`
2. refuse if the target month file is ≥ 50 MiB (compact first)
3. if the file does not end in `\n`, prepend `\n` (seals a torn line off)
4. write the whole line plus `\n` in one write
5. release the lock

A line counts as written only once its `\n` is. There is no fsync — git is the durability layer.
Only files nobody appends to any more (past months, compact, `_import`) are ever rewritten, and only with tmp-then-rename.

## Readers

Reading never fails. Take no lock; for every `*.jsonl` under `log/`:

- strip a BOM, accept CRLF, replace invalid UTF-8
- ignore the text after the last `\n` (a write in progress or a torn write)
- skip blank lines and merge-conflict markers (`<<<<<<<` `=======` `|||||||` `>>>>>>>`) — keep the rows on both sides
- skip a line that isn't a JSON object, or whose known fields have the wrong type, and report `file:line`
- **keep every field and kind you don't know**, and write them back unchanged
- drop duplicate `id`s, keeping the first in path order (a union merge duplicates lines)
- a row without `files` (legacy) is valid to read

A row is hidden by default when a close row's `ref` names it, it has a `closed` field, or another row `supersedes` it.
