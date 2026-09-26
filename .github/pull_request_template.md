## What

<!-- What this PR changes and why. Link the issue: Fixes #123 -->

## How it was tested

<!-- Commands run, cases covered. -->

## Checklist

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `scripts/file-size.sh`
- [ ] `cargo test --workspace --locked`
- [ ] Conventional commits, one concern per commit
- [ ] Touches the hook path? Measured with `hyperfine` before/after
- [ ] Changes the log format? `docs/format.md` updated (it is a spec change)
