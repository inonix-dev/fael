use fael_core::*;

#[test]
fn ids_and_clock() {
    let a = ulid_at(1_000);
    let b = ulid_at(2_000);
    assert_eq!(a.len(), 26);
    assert!(a < b, "ULIDs sort by time");
    assert!(
        a.bytes()
            .all(|c| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&c))
    );
    assert_ne!(ulid_at(5), ulid_at(5), "random part differs within one ms");
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(1_790_330_400_000), "2026-09-25T10:00:00.000Z");
    assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(rfc3339(1_790_330_400_123), "2026-09-25T10:00:00.123Z");
    assert_eq!(ts_ms("2026-09-25T10:00:00.123Z"), Some(1_790_330_400_123));
    assert_eq!(ts_ms("2026-09-25T10:00:00Z"), Some(1_790_330_400_000));
}

#[test]
fn writer_id_hides_email() {
    let a = writer_id("Dela Mind", Some("Kire@Example.com"), "host");
    let b = writer_id("dela.mind", Some("kire@example.com"), "other");
    assert_eq!(a[..a.len() - 5], *"dela-mind");
    assert_eq!(
        a[a.len() - 4..],
        b[b.len() - 4..],
        "same email, any case → same hash"
    );
    assert_ne!(a, writer_id("Dela Mind", Some("x@y.z"), "host"));
    assert!(!a.contains("example"));
    assert!(
        writer_id("__", None, "h").starts_with("anon-"),
        "never starts with _"
    );
}
