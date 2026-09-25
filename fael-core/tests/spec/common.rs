use fael_core::*;
use std::fs;
use std::path::PathBuf;

pub(crate) fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-test-{}", ulid()));
    fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn row(kind: &str, files: &[&str]) -> Row {
    Row::new(
        "tester-0000",
        kind,
        "some standalone text",
        files.iter().map(|s| s.to_string()).collect(),
    )
}
