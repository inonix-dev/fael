# Contributing to fael

Thanks for your interest in contributing!

## Development setup

```bash
git clone https://github.com/inonix-dev/fael.git
cd fael
cargo test --workspace
```

Rust 1.89 or newer. The workspace has two crates: `fael-core` (log format, find, rules — no git,
no cwd) and `fael` (CLI, MCP server, hooks, install).

## Before submitting

The same checks CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

- **Conventional commits** — `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `ci:`, `chore:`
- **One concern per commit**
- Open the PR against `main` from your branch; merging is the maintainer's call

## Project rules

- **Hooks stay fast** — every hook is a process spawn on every tool call (about 2–3 ms today).
  Measure with `hyperfine` before and after anything that touches the hook path.
- **Hooks fail open** — `fael hook …` always exits 0 and never blocks an agent because fael itself broke.
- **The log format is a contract** — [docs/format.md](docs/format.md) is versioned; a change there is
  a spec change, not a refactor. `.fael/` holds shared project data only; per-machine runtime state
  lives in `~/.local/state/fael/`.
- **Rules live in core, clients live in adapters** — see [docs/architecture.md](docs/architecture.md).
  Adding a client touches only an adapter.
- **No abstraction for abstraction's sake** — no interface with one implementation, no scaffolding
  for a future that may not come.
- **Prefer std** — a new dependency needs a reason a few lines of code can't cover.

## Releasing (maintainers)

```bash
scripts/release.sh          # 0.0.1 -> 0.0.2
scripts/release.sh minor    # 0.0.2 -> 0.1.0
```

It bumps `fael/Cargo.toml`, commits `release vX.Y.Z` on `main`, tags it and pushes. The tag runs
`.github/workflows/release.yml`, which builds every platform and publishes the GitHub Release, npm
`@inonix/fael` and the Homebrew formula together.
