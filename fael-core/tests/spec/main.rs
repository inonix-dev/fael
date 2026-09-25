//! SPEC-fael-v0 §10 fail examples + §11 read-side healing, against the real files.
//!
//! Thin entry only — the suites sit next to this file:
//! `common` (tmp/row helpers), `validate` (§10 rejections), `append` (write
//! path + atomicity), `parse` (§11 read-side healing), `ids` (ULID/clock/ids).

mod append;
mod common;
mod ids;
mod parse;
mod validate;
