//! `fael hook <stop|session-start|read|edit> [--client c]` — stdin in, stdout out.
//! The decision (`core::decide_stop`, `core::push`) is written once; each
//! adapter only parses its client's JSON and renders the answer back.
//! No `--client` = the neutral protocol from SPEC §9: Event in, Reply out.
//! Adapters: `claude`, `codex` (same hook shape; codex hands stop its last
//! message and edits as apply_patch). OpenCode's plugin speaks neutral.
//!
//! Thin entry only — the events live in `hook/`:
//! `protocol` (neutral Event/Reply + shared ctx), `claude` (client adapters),
//! `stop` (turn-end work/bug rule), `session` (session-start kickoff),
//! `push` (read/edit context), `state` (per-machine session files),
//! `usage` (SPEC §8 accounting + `stats`), `markers` (bug phrases).
//!
//! The hook always exits 0. Any internal error is an empty Reply (let the
//! turn through) — a memory tool must never break the agent's tool call.

mod claude;
mod markers;
mod protocol;
mod push;
mod session;
mod state;
mod stop;
mod usage;

pub(crate) use protocol::cmd;
pub(crate) use usage::stats;

// What the binary shares: `write` reads session edits and checks anchors,
// `maintain` asks where the gitignore rule comes from.
pub(crate) use push::is_anchor;
pub(crate) use session::{deliberate, ignore_source};
pub(crate) use state::{Edit, session_edits, state_dir};
