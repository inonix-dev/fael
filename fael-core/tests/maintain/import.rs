use crate::common::*;
use fael_core::*;
use std::fs;

fn legacy(kind: &str, id: &str, files: &str) -> String {
    format!(
        r#"{{"ts":"2026-01-01T00:00:00Z","agent":"delamind","id":"{id}","kind":"{kind}","text":"legacy {id}","files":[{files}],"v":2}}"#
    )
}

#[test]
fn fapony_legacy_mapping() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("old-memory");
    fs::create_dir_all(&src).unwrap();
    let lines = [
        legacy("decision", "muft0001", r#""src/a.rs""#),
        legacy("bug", "muft0002", r#""src/b.rs""#),
        legacy("next", "muft0003", r#""src/c.rs""#), // a cut kind → note + legacy_kind
        legacy("close", "muft0004", r#""src/a.rs""#).replace(r#""kind":"close""#, r#""kind":"close","ref":"muft0001""#),
        legacy("close", "muft0005", r#""src/a.rs""#).replace(r#""kind":"close""#, r#""kind":"close","ref":"missing-id""#),
        r#"{"ts":"2026-01-02T00:00:00Z","agent":"delamind","kind":"note","text":"no id here","files":["src/d.rs"],"v":2}"#.to_string(),
        r#"{"ts":"2026-01-03T00:00:00Z","agent":"delamind","id":"muft0007","kind":"note","text":"no files here","v":2}"#.to_string(),
        r#"{"ts":"2026-01-04T00:00:00Z","agent":"delamind","id":"muft0008","kind":"note","text":"anchor stays","files":["doc:pricing"],"v":2}"#.to_string(),
    ];
    fs::write(src.join("log.delamind.jsonl"), lines.join("\n") + "\n").unwrap();
    let rep = import(&fael, &src, &[], &ImportOpts::default()).unwrap();
    assert_eq!(
        (rep.adds, rep.folded, rep.carried, rep.skipped),
        (6, 1, 1, 0),
        "{rep:?}"
    );
    assert_eq!(rep.paths.len(), 2); // .jsonl + .close.jsonl companion
    let log = read(&fael);
    let by_id = |id: &str| log.rows.iter().find(|x| x.id == id).unwrap().clone();
    assert_eq!(by_id("muft0001").by, "delamind");
    assert_eq!(by_id("muft0002").kind, "issue");
    let n = by_id("muft0003");
    assert_eq!(n.kind, "note");
    assert_eq!(
        n.extra.get("legacy_kind").and_then(|v| v.as_str()),
        Some("next")
    );
    assert!(closed(&log).contains("muft0001")); // the close folded in
    assert!(by_id("muft0001").extra.contains_key("closed"));
    assert_eq!(log.closes.len(), 1); // the unresolvable close rides the companion
    let noid = log
        .rows
        .iter()
        .find(|x| x.id.starts_with("legacy-"))
        .unwrap();
    assert_eq!(noid.text, "no id here");
    let nofiles = by_id("muft0007");
    assert!(nofiles.files.is_empty()); // kept as-is, read-valid
    assert_eq!(by_id("muft0008").files, vec!["doc:pricing"]);
    // importing twice is safe — dedupe by id
    let rep2 = import(&fael, &src, &[], &ImportOpts::default()).unwrap();
    assert_eq!(rep2.adds, 6);
    assert_eq!(read(&fael).rows.len(), log.rows.len());
}

#[test]
fn import_map_rewrites_prefixes_not_anchors() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("old");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("log.jsonl"),
        legacy("decision", "muft0001", r#""old/svc/a.rs","doc:pricing""#) + "\n",
    )
    .unwrap();
    let rep = import(
        &fael,
        &src,
        &[],
        &ImportOpts {
            maps: vec![("old/".into(), "new/".into())],
        },
    )
    .unwrap();
    assert_eq!(rep.adds, 1);
    let row = &read(&fael).rows[0];
    assert_eq!(row.files, vec!["new/svc/a.rs", "doc:pricing"]);
}

#[test]
fn import_spec_fills_empty_files() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("old");
    fs::create_dir_all(&src).unwrap();
    let lines = [
        r#"{"ts":"2026-07-30T05:31:40.114Z","agent":"a","id":"mus00001","kind":"decision","text":"t","spec":"old/PLAN-x.md"}"#,
        r#"{"ts":"2026-07-30T05:31:40.114Z","agent":"a","id":"mus00002","kind":"decision","text":"t","spec":"PLAN-x chunk 3 notes"}"#,
        r#"{"ts":"2026-07-30T05:31:40.114Z","agent":"a","id":"mus00003","kind":"decision","text":"t","spec":"PLAN-y.md","files":["a.rs"]}"#,
        r#"{"ts":"2026-07-30T05:31:40.114Z","id":"mus00004","kind":"bug","text":"t","spec":"old/PLAN-z.md"}"#,
        r#"{"ts":"2026-07-30T05:31:40.114Z","agent":"a","id":"mus00005","kind":"synced","text":"","spec":"old/PLAN-z.md"}"#,
        r#"{"ts":"2026-07-30T05:31:41.114Z","kind":"close","id":"mus00004","text":"done"}"#,
    ];
    fs::write(src.join("log.jsonl"), lines.join("\n") + "\n").unwrap();
    let opts = ImportOpts {
        maps: vec![("old/".into(), "new/".into())],
    };
    import(&fael, &src, &[], &opts).unwrap();
    let rows = read(&fael).rows;
    let files = |id: &str| rows.iter().find(|r| r.id == id).unwrap().files.clone();
    assert_eq!(files("mus00001"), vec!["new/PLAN-x.md"]);
    assert!(files("mus00002").is_empty());
    assert_eq!(files("mus00003"), vec!["a.rs"]);
    // no `agent` is still fapony: mapped kind, writer, spec
    let r4 = rows.iter().find(|r| r.id == "mus00004").unwrap();
    assert_eq!((r4.kind.as_str(), r4.by.as_str()), ("issue", "legacy"));
    assert_eq!(files("mus00004"), vec!["new/PLAN-z.md"]);
    assert!(files("mus00005").is_empty());
    // a close naming its target in `id` folds, not a duplicate
    assert_eq!(rows.iter().filter(|r| r.id == "mus00004").count(), 1);
    assert!(r4.extra.contains_key("closed"));
}

#[test]
fn import_native_fael_log() {
    let r = root();
    let fael = fael_of(&r);
    let other = tmp().join("other").join(".fael").join("log");
    fs::create_dir_all(other.join("w-0000")).unwrap();
    let line = row("A0000000000000000000000001", "decision", &["x.rs"]).to_line();
    fs::write(
        other.join("w-0000").join("2026-01.jsonl"),
        line.clone() + "\n",
    )
    .unwrap();
    let rep = import(&fael, &other, &[], &ImportOpts::default()).unwrap();
    assert_eq!((rep.adds, rep.skipped), (1, 0));
    assert_eq!(read(&fael).rows.len(), 1);
}

#[test]
fn import_350_legacy_rows_drops_nothing() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("big");
    fs::create_dir_all(&src).unwrap();
    let mut lines = vec![];
    for i in 0..350 {
        let kind = if i % 10 == 0 {
            "bug"
        } else if i % 15 == 0 {
            "next"
        } else {
            "note"
        };
        if i % 70 == 0 {
            lines.push(format!(
                r#"{{"ts":"2026-01-01T00:00:00Z","agent":"w","kind":"{kind}","text":"row {i}","files":["f{i}.rs"],"v":2}}"#
            )); // no id → legacy-…
        } else {
            lines.push(legacy(kind, &format!("id{i:04}"), &format!(r#""f{i}.rs""#)));
        }
    }
    for (c, t) in [("c0001", "id0001"), ("c0002", "id0002"), ("c0003", "nope")] {
        lines.push(format!(
            r#"{{"ts":"2026-01-02T00:00:00Z","agent":"w","id":"{c}","kind":"close","ref":"{t}","text":"done","files":[],"v":2}}"#
        ));
    }
    fs::write(src.join("log.jsonl"), lines.join("\n") + "\n").unwrap();
    let rep = import(&fael, &src, &[], &ImportOpts::default()).unwrap();
    assert_eq!(rep.skipped, 0, "{:?}", rep.warnings);
    assert_eq!(rep.adds + rep.folded + rep.carried, lines.len(), "{rep:?}");
    assert_eq!(rep.folded, 2);
    assert_eq!(rep.carried, 1);
}
