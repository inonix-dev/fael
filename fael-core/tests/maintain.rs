//! Chunk 6: `doctor --fix` per SPEC §11 case, `compact`, `import` — against
//! real files in throwaway dirs (months are injected, never the wall clock).
//!
//! Thin entry only — the suites live in `tests/maintain/`:
//! `common` (shared helpers), `doctor_scan`, `doctor_fix`, `compact`, `import`.

#[path = "maintain/common.rs"]
mod common;
#[path = "maintain/compact.rs"]
mod compact;
#[path = "maintain/doctor_fix.rs"]
mod doctor_fix;
#[path = "maintain/doctor_scan.rs"]
mod doctor_scan;
#[path = "maintain/import.rs"]
mod import;
