# Security Policy

## Reporting a vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Use GitHub's private vulnerability reporting instead:
[Report a vulnerability](https://github.com/zecalis/fael/security/advisories/new).

## Scope

fael is a local CLI, a stdio MCP server and a set of agent hooks. It opens no network port and
talks to no server. The main concerns are:

- **Path handling** — every file path in a row goes through `normalize_files`: paths outside the
  repo or climbing past its root with `..` are rejected. A way to make fael read or write outside
  the repo is a vulnerability.
- **Config written by `fael install`** — install edits `~/.claude/settings.json`, `~/.codex/` and
  `~/.config/opencode/`. It must only add or repoint fael's own entries, and remove fapony's only with `--replace-fapony`.
- **Rows are untrusted text** — the log is shared through git, so a row may be written by anyone
  who can push to the repo. fael hands rows to agents as context; treat a repo's `.fael/log` with the
  same trust as its code.
- **Supply chain** — release binaries are built by GitHub Actions from a tagged commit, and the npm
  package is published with provenance (Trusted Publishing). Report anything that breaks that chain.

## Supported versions

Only the latest release gets fixes while fael is 0.x.

| Version | Supported |
|---------|-----------|
| 0.0.x   | Yes       |
