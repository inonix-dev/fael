//! SPEC-fael-v0 §10 fail examples + §11 read-side healing, against the real files.
//!
//! Thin entry only — the suites live in `tests/spec/`:
//! `common` (tmp/row helpers), `validate` (§10 rejections), `append` (write
//! path + atomicity), `parse` (§11 read-side healing), `ids` (ULID/clock/ids).

#[path = "spec/common.rs"]
mod common;
#[path = "spec/validate.rs"]
mod validate;
#[path = "spec/append.rs"]
mod append;
#[path = "spec/parse.rs"]
mod parse;
#[path = "spec/ids.rs"]
mod ids;
