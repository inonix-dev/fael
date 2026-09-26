//! fael-core — the log format (docs/format.md): row v1, validate, read, append under lock.
//! Knows the row format, never a client. The CLI, MCP and hooks sit on top of this.
//!
//! Thin entry only: module wiring, the public surface (every name the old flat
//! `lib.rs` exported still resolves here), the size/key consts, and `anchor`
//! — the one primitive every matcher shares. The row shape lives in `row`,
//! repo settings in `config`, write-time checks in `validate`.

mod aliases;
mod compact;
mod config;
mod doctor;
mod hook;
mod id;
mod import;
mod log;
mod query;
mod row;
mod validate;

pub use aliases::{Aliases, is_alias_row};
pub use compact::{Opts as CompactOpts, Report as CompactReport, WriterReport, compact};
pub use config::Config;
pub use import::{Opts as ImportOpts, Report as ImportReport, import};

pub use doctor::{
    Kind as ProblemKind, Problem, Report as DoctorReport, Severity, current_month,
    fix as doctor_fix, scan as doctor_scan,
};

pub use hook::{StopFacts, decide_stop, last_row_ms};

pub use id::{now_ms, rfc3339, to_matches, ts_ms, ulid, ulid_at, writer_id};
pub use log::{
    Log, MONTH_MAX, add, add_row, append, bump_row, close, close_row, mv_row, parse, read,
};
pub use query::{
    Cut, Filter, KeyUse, Urgent, UrgentChange, abbrev, brief, closed, cmp_rows, est_tokens, find,
    fresh_ts, glob, gone, keys, kickoff, page, push, query, ranked, render, render_full,
    render_full_page, render_page, resolve, resolve_urgent, superseded, warnings,
};
pub use row::{Row, Stamp};
pub use validate::{normalize_files, valid_key, validate, validate_alias, validate_close};

/// Core kinds with fixed meaning; a repo adds more through `Config::kinds`.
pub const CORE_KINDS: [&str; 3] = ["decision", "issue", "note"];
/// Hard cap on one serialised row, in bytes (Thai is 3 bytes/char).
pub const ROW_BYTES_MAX: usize = 10 * 1024;
pub const KEY_MAX: usize = 64;

/// `scheme:ref` anchor (`doc:pricing`, `issue:#12`): a scheme of ≥ 2 chars `[a-z0-9+.-]`, starting
/// with a letter, before the first `:` and before any `/`. Two chars minimum so `C:` stays a drive.
/// Returns the ref — opaque to fael (`/` in it is not a path separator); it must be non-empty.
// ponytail: a root-level file named like `notes:v2.md` reads as an anchor — rare, rename the file
pub(crate) fn anchor(f: &str) -> Option<&str> {
    f.split_once(':')
        .filter(|(s, _)| {
            s.len() >= 2
                && s.as_bytes()[0].is_ascii_lowercase()
                && s.bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'+' | b'.' | b'-'))
        })
        .map(|(_, r)| r)
}
