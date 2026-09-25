# fael

[![npm](https://img.shields.io/npm/v/@inonix/fael.svg)](https://www.npmjs.com/package/@inonix/fael) [![release](https://img.shields.io/github/v/release/inonix-dev/fael.svg)](https://github.com/inonix-dev/fael/releases)

**A repo's memory that agents can't skip writing.** fael keeps decisions, bugs and notes in
`.fael/` inside the repo, so git carries them to everyone who clones it. Hooks push the rows for a
file back to the agent when it touches that file, and stop the turn when it edited files without
writing anything down. Works with Claude Code, Codex and OpenCode. No server, no account.

## Install

**1. Get the binary** — prebuilt for macOS, Linux and Windows; none of these need Rust.

```bash
npm i -g @inonix/fael                 # Node
brew install inonix-dev/tap/fael      # Homebrew (macOS / Linux)
curl -LsSf https://github.com/inonix-dev/fael/releases/latest/download/fael-installer.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/inonix-dev/fael/releases/latest/download/fael-installer.ps1 | iex
```

From source: `cargo install --git https://github.com/inonix-dev/fael fael`.

**2. Wire it into your agents** — once per machine.

```bash
fael install              # finds Claude Code, Codex and OpenCode and adds hooks, MCP and a skill
fael install --dry-run    # show what it would change, write nothing
```

`fael` has to be on your `PATH`: the hooks call it by name, so an upgrade never leaves them pointing
at an old path. That is also why `npx @inonix/fael install` is refused — npx keeps the binary in a
throwaway cache. Install it globally first.

Coming from fapony? `fael install --replace-fapony` removes fapony's Stop/session-start hooks and MCP
server so the two don't both block, and `fael import .fapony/.memory` brings the old log over.

**3. Use it in a repo** — the first row adopts the repo (creates `.fael/log/`); commit it like code.

```bash
fael add decision "what was locked, and why" --files src/x.rs
fael add bug "what is broken" --files src/x.rs
fael close <id> "fixed in <sha>"
fael find --files src/x.rs
fael kickoff              # what the next session needs to know
```

Agents do the same through the `fael` MCP server. `fael doctor` checks the setup (for example a
gitignored `.fael/log`), `fael` with no arguments lists every command.

## Docs

- [architecture.md](docs/architecture.md) — how it fits together
- [format.md](docs/format.md) — the log format, for tools that read or write it without fael
- [integrate.md](docs/integrate.md) — wire fael into another agent

## License

MIT
