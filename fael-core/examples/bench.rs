//! SPEC §8: full-parse cost of `read()` at 10k / 100k / 300k synthetic rows.
//! `cargo run --release -p fael-core --example bench`

use fael_core::*;
use std::fs;
use std::time::Instant;

fn main() {
    for n in [10_000usize, 100_000, 300_000] {
        let dir = std::env::temp_dir().join(format!("fael-bench-{}", ulid()));
        // 5 writers × 12 months, ~430-byte rows (fapony's real average: 150 KB / 350 rows)
        let mut bytes = 0;
        for w in 0..5 {
            for m in 1..=12 {
                let mut buf = String::new();
                for i in 0..n / 60 {
                    let mut r = Row::new(
                        &format!("writer{w}-0000"),
                        ["decision", "issue", "note"][i % 3],
                        &"word ".repeat(60),
                        vec![format!("src/mod{}/file{}.rs", i % 40, i % 300)],
                    );
                    r.ts = format!("2026-{m:02}-01T00:00:00Z");
                    r.key = (i % 5 == 0).then(|| format!("area{}:sub{}", i % 7, i % 3));
                    buf.push_str(&r.to_line());
                    buf.push('\n');
                }
                let d = dir.join(format!("log/writer{w}-0000"));
                fs::create_dir_all(&d).unwrap();
                bytes += buf.len();
                fs::write(d.join(format!("2026-{m:02}.jsonl")), buf).unwrap();
            }
        }
        read(&dir); // warm the page cache
        let mut times: Vec<f64> = (0..5)
            .map(|_| {
                let t = Instant::now();
                let log = read(&dir);
                assert!(log.rows.len() >= n - 60);
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        times.sort_by(f64::total_cmp);
        println!(
            "{n:>7} rows  {:>6.1} MB  read median {:>7.1} ms  max {:>7.1} ms",
            bytes as f64 / 1e6,
            times[2],
            times[4]
        );
        fs::remove_dir_all(&dir).ok();
    }
}
