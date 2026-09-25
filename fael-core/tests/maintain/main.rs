//! Chunk 6: `doctor --fix` per SPEC §11 case, `compact`, `import` — against
//! real files in throwaway dirs (months are injected, never the wall clock).
//!
//! Thin entry only — the suites sit next to this file:
//! `common` (shared helpers), `doctor_scan`, `doctor_fix`, `compact`, `import`.

mod common;
mod compact;
mod doctor_fix;
mod doctor_scan;
mod import;
