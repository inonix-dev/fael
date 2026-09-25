//! ids and clocks: ULID, RFC 3339 timestamps, writer id — std only apart from the OS RNG and sha256.

use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A ULID: 48-bit ms timestamp + 80 random bits, 26 chars of Crockford base32.
pub fn ulid() -> String {
    ulid_at(now_ms())
}

// ponytail: no per-ms monotonic counter — two rows in the same ms from one process sort randomly;
// add the counter if causal order inside one ms ever matters
pub fn ulid_at(ms: u64) -> String {
    let mut rnd = [0u8; 16];
    getrandom::fill(&mut rnd[6..]).expect("OS random source");
    let n = ((ms as u128 & 0xFFFF_FFFF_FFFF) << 80) | u128::from_be_bytes(rnd);
    (0..26)
        .rev()
        .map(|i| CROCKFORD[((n >> (5 * i)) & 31) as usize] as char)
        .collect()
}

/// `2026-09-25T10:00:00Z` for a unix time in ms (UTC).
pub fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, sod) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        sod / 60 % 60,
        sod % 60
    )
}

/// `<slug of name>-<4 hex of sha256(lowercase email)>`; no email → hash `host` instead.
/// The email itself never reaches the repo — only the hash.
pub fn writer_id(name: &str, email: Option<&str>, host: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "anon" } else { slug };
    let seed = email.map_or_else(|| host.to_string(), str::to_lowercase);
    let h = Sha256::digest(seed.as_bytes());
    format!("{slug}-{:02x}{:02x}", h[0], h[1])
}
