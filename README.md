# fael

[![npm](https://img.shields.io/npm/v/@inonix/fael.svg)](https://www.npmjs.com/package/@inonix/fael)
[![release](https://img.shields.io/github/v/release/inonix-dev/fael.svg)](https://github.com/inonix-dev/fael/releases)
[![CI](https://github.com/inonix-dev/fael/actions/workflows/ci.yml/badge.svg)](https://github.com/inonix-dev/fael/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Every agent on your repo knows what was decided and what's still open — without re-asking, and
without a context dump.**

One person runs five agents; a team runs fifty. Each one starts from zero: it finds the same flaky
test, re-asks why that function looks weird, and repeats the mistake the last agent already fixed.
Memory tools that try to help stuff a summary of everything into context before the agent has said
what it's about to do — you pay tokens for noise, and the one row that mattered gets averaged away.

fael works the other way round: the agent **can't finish without writing**, and when it opens a
file it gets **only what was written about that file**. The file it touches is the question.

fael gives the repo a memory that agents can't skip:

- **They have to write.** When an agent edited files but recorded nothing, fael stops the turn and
  asks for a note. Mention a bug without filing it? Same.
- **Memory finds them.** When an agent reads a file, the decisions and open bugs about *that file*
  come attached — nobody has to remember to search.
- **No spam in context.** Rows are pushed per file, once per session, and cut to a token budget
  (800 by default) — not a notes dump. Anything else the agent asks for itself, through MCP.
  `fael stats` shows exactly what fael has put into context.
- **It follows the code.** Rename a file and its rows follow it (`git log -M`). Split one into
  several and `fael mv old new` points the rows at the new files.
- **It lives in git.** Rows are plain JSONL in `.fael/`. Clone the repo and you get every decision,
  bug and note with it — for every agent and every person on the team. No server, no account.

Works with **Claude Code, Codex and OpenCode**, and any MCP host. One small binary; hooks run in 2–3 ms.

## What you get

| Without fael | With fael |
|---|---|
| Each session starts from zero | Session opens with what the last one left: open bugs, recent decisions |
| "Why is it like this?" — ask again, guess again | The reason sits next to the file, from the agent that made the call |
| Agent notices a bug mid-task, then forgets it | It's filed on the spot, and shown to whoever touches that file next |
| Knowledge stays in one person's chat history | It's in the repo — teammates and their agents get it on `git pull` |

## Install

**1. Get the binary** — prebuilt for macOS, Linux and Windows. No Rust needed.

```bash
npm i -g @inonix/fael               # Node
brew install inonix-dev/tap/fael    # Homebrew (macOS / Linux)
curl -LsSf https://github.com/inonix-dev/fael/releases/latest/download/fael-installer.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/inonix-dev/fael/releases/latest/download/fael-installer.ps1 | iex
```

**2. Connect your agents** — once per machine.

```bash
fael install              # finds Claude Code, Codex and OpenCode; adds hooks, MCP server and a skill
fael install --dry-run    # show what would change, write nothing
```

`fael` must be on your `PATH` — the hooks call it by name, so upgrades never leave them pointing at
an old path. That's also why `npx @inonix/fael install` is refused: npx keeps the binary in a
throwaway cache. Install it globally first.

**3. Work as usual.** The first row an agent writes creates `.fael/log/` in the repo — commit it like code.

## How it works

```
agent reads src/pay.rs   →  fael attaches: "[bug] refund rounds down on JPY → src/pay.rs"
agent edits src/pay.rs   →  fael notes the edit
agent tries to finish    →  fael: "you edited src/pay.rs — record what changed or what you found"
agent                    →  fael add decision "refunds round half-up, per finance" --files src/pay.rs
git push                 →  the next agent, on any machine, sees it when it opens src/pay.rs
```

Agents use the `fael` MCP server (`find`, `add`, `close`). You can use the same log from the shell:

```bash
fael kickoff                                             # what this session should know
fael find --files src/pay.rs                             # everything about one file
fael add bug "refund rounds down on JPY" --files src/pay.rs
fael add issue "count from order date or ship date?" --to finance --files src/pay.rs
fael close <id> "fixed in 4f2a91c"
fael mv src/pay.rs src/pay/refund.rs                     # a split git can't see — rows follow
fael doctor                                              # check the setup (e.g. a gitignored .fael/log)
```

`fael` with no arguments lists every command.

**Coming from fapony?** `fael install --replace-fapony` switches the hooks over, and
`fael import .fapony/.memory` brings the old log with it — no row lost.

## Links

- **npm:** [@inonix/fael](https://www.npmjs.com/package/@inonix/fael)
- **Homebrew tap:** [inonix-dev/homebrew-tap](https://github.com/inonix-dev/homebrew-tap)
- **Releases & changelog:** [GitHub Releases](https://github.com/inonix-dev/fael/releases)
- **Bugs & ideas:** [Issues](https://github.com/inonix-dev/fael/issues)
- **Docs:** [architecture](docs/architecture.md) · [log format](docs/format.md) (read/write it without fael) · [integrate another agent](docs/integrate.md)
- [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [Code of Conduct](CODE_OF_CONDUCT.md)

## License

[MIT](LICENSE) © inonix
