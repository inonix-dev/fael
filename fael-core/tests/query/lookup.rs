//! resolve · keys · warnings · glob — finding rows and keys by name.

use super::{log, row};
use fael_core::*;

#[test]
fn redis_glob() {
    assert!(glob("auth:*", "auth:session:timeout"));
    assert!(glob("a?c", "abc") && !glob("a?c", "ac"));
    assert!(glob("h[ae]llo", "hello") && !glob("h[ae]llo", "hillo"));
    assert!(glob("h[^e]llo", "hallo") && !glob("h[^e]llo", "hello"));
    assert!(glob("v[0-9]", "v7") && !glob("v[0-9]", "vx"));
    assert!(glob("a\\*", "a*") && !glob("a\\*", "ab"));
    assert!(!glob("auth:*", "billing:x"));
}

#[test]
fn resolve_by_unique_prefix() {
    let l = log();
    assert_eq!(resolve(&l, "b0").unwrap().id, "B0000000000000000000000015"); // case-insensitive
    assert!(resolve(&l, "A000").unwrap_err().contains("matches 5 rows"));
    assert!(resolve(&l, "Z").unwrap_err().contains("no row"));
    assert!(resolve(&l, "").is_err());
}

#[test]
fn keys_by_count() {
    let mut l = log();
    l.rows.push(row(
        "C0000000000000000000000016",
        "note",
        &["x"],
        Some("billing:invoice"),
    ));
    l.rows.push(row(
        "C0000000000000000000000017",
        "note",
        &["x"],
        Some("billing:invoice"),
    ));
    let k = keys(&l, None);
    assert_eq!(k[0].key, "billing:invoice");
    assert_eq!(k[0].count, 3);
    assert_eq!(k[0].last, "2026-09-17T00:00:00Z");
    assert_eq!((k[1].key.as_str(), k[1].count), ("auth:session", 2));
    assert_eq!(keys(&l, Some("auth:*")).len(), 1);
}

#[test]
fn add_warnings_never_reject() {
    let l = log();
    let cfg = Config {
        key_domains: vec!["auth".into()],
        warn_row_tokens: 3,
        ..Config::default()
    };
    let mut r = row(
        "D0000000000000000000000017",
        "note",
        &["x"],
        Some("auth:sesion"),
    );
    let w = warnings(&r, &l, &cfg);
    assert!(w[0].contains("similar keys exist: auth:session"), "{w:?}");
    assert!(w[1].contains("tokens (warn at 3)"), "{w:?}");
    r.key = Some("hiring:backend".into());
    assert!(warnings(&r, &l, &cfg)[0].contains("not in config key_domains"));
    r.key = Some("auth:session".into()); // existing key: no similarity noise
    r.text = "ok".into();
    assert!(warnings(&r, &l, &cfg).is_empty());
}
