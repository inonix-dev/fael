//! Usage accounting: temp-dir repos are skipped unless the state dir is
//! scratch too.

use super::{fael_at, json};
use std::path::Path;

#[test]
fn stats_skips_temp_repos_unless_state_is_scratch_too() {
    // a real state dir (outside the OS temp dir) drops usage from temp-dir
    // benchmark repos (01M3CRR6A)
    let state = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stats-{}", fael_core::ulid()));
    std::fs::create_dir_all(&state).unwrap();
    let tmp_repo = std::env::temp_dir().join("faelbench.x");
    let line = |repo: &Path| {
        format!(
            r#"{{"ts":"2026-09-26T00:00:00.000Z","repo":{},"client":"claude","event":"read","bytes":10,"est_tokens":3,"ids":["A"]}}"#,
            json(repo)
        )
    };
    std::fs::write(
        state.join("usage.jsonl"),
        format!("{}\n{}\n", line(&tmp_repo), line(Path::new("/work/real"))),
    )
    .unwrap();
    let (ok, out, _) = fael_at(&state, &state, &["stats", "--json"], "");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(ok && v["events"] == 1 && v["skipped_temp"] == 1, "{out}");
}
