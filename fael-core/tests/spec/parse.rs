use fael_core::*;

#[test]
fn read_heals_bom_crlf_conflicts_junk() {
    let a = r#"{"v":1,"id":"A","ts":"2026-09-01T00:00:00Z","by":"w","kind":"note","text":"a","files":["x"]}"#;
    let b = r#"{"v":1,"id":"B","ts":"2026-09-01T00:00:00Z","by":"w","kind":"note","text":"b","files":["x"]}"#;
    let raw = format!("\u{feff}{a}\r\n<<<<<<< HEAD\r\n{b}\n=======\nnot json\n>>>>>>> dev\n\n");
    let (mut rows, mut warn) = (vec![], vec![]);
    fael_core::parse(raw.as_bytes(), "f.jsonl", &mut rows, &mut warn);
    assert_eq!(
        rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(warn.len(), 1);
    assert!(warn[0].starts_with("f.jsonl:5: broken line"), "{warn:?}");

    let (mut rows, mut warn) = (vec![], vec![]);
    fael_core::parse(b"\xff\xfe{\"id\":1}\n", "g.jsonl", &mut rows, &mut warn); // bad utf-8 + wrong type
    assert!(rows.is_empty() && warn.len() == 1);
}

#[test]
fn unknown_fields_and_legacy_rows_round_trip() {
    let line = r#"{"id":"legacy-1a2b3c4d","ts":"2026-01-01T00:00:00Z","by":"old","kind":"claim","text":"t","sha":"abc","closed":{"id":"C","text":"x"},"future":[1,2]}"#;
    let r: Row = serde_json::from_str(line).unwrap();
    assert_eq!(r.v, None);
    assert!(r.files.is_empty());
    let back: serde_json::Value = serde_json::from_str(&r.to_line()).unwrap();
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(line).unwrap()
    );
}
