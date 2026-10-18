//! Ids and time (docs/research/m6-api.md, M6-55, HX7, OP11): random ids,
//! a prefix and 26 lower-case base32 characters of 128 random bits, and
//! Unix milliseconds from `SystemTime`. Under `test-support` both can be
//! fixed, so goldens are exact (VT6).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Crockford's base32 alphabet, lower case.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// 128 bits as 26 base32 characters, most significant first.
pub fn base32(value: u128) -> String {
    (0..26)
        .rev()
        .map(|i| {
            let index = (value >> (i * 5)) & 0x1f;
            char::from(ALPHABET[usize::try_from(index).unwrap_or(0)])
        })
        .collect()
}

/// True if `text` is an id of `prefix` (M6-55): the prefix, `_` and 26
/// characters of the alphabet (HX5: anything else names nothing).
pub fn well_formed(prefix: &str, text: &str) -> bool {
    text.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|body| body.len() == 26 && body.bytes().all(|b| ALPHABET.contains(&b)))
}

/// Where ids and times come from.
#[derive(Debug, Default)]
pub struct Ids {
    /// Under `test-support`: the next fixed id's number; 0 means random.
    next: AtomicU64,
    /// Under `test-support`: a fixed time; 0 means the system's clock.
    fixed_ms: AtomicI64,
}

impl Ids {
    /// 128 random bits from the operating system (`getrandom`).
    ///
    /// # Panics
    ///
    /// If the operating system has no random source, which no supported
    /// platform lacks; an id must never be guessable.
    #[allow(clippy::expect_used)]
    pub fn random_bits() -> u128 {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).expect("the operating system's random source");
        u128::from_be_bytes(bytes)
    }

    /// 32 random bytes for a token's secret (AU3); under `test-support`
    /// with fixed ids, bytes derived from the next id's number, so goldens
    /// are exact (OP11).
    ///
    /// # Panics
    ///
    /// As [`Ids::random_bits`]: a token must never be guessable.
    #[allow(clippy::expect_used)]
    pub fn secret_bytes(&self) -> [u8; 32] {
        let fixed = self.next.load(Ordering::Relaxed);
        if fixed == 0 {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).expect("the operating system's random source");
            return bytes;
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        fitment_core::sha256(format!("fitment-test-secret {n}").as_bytes())
    }

    /// A new id: `prefix` and 26 base32 characters (M6-55).
    pub fn id(&self, prefix: &str) -> String {
        let fixed = self.next.load(Ordering::Relaxed);
        let bits = if fixed == 0 {
            Self::random_bits()
        } else {
            u128::from(self.next.fetch_add(1, Ordering::Relaxed))
        };
        format!("{prefix}_{}", base32(bits))
    }

    /// Now, in Unix milliseconds.
    pub fn now_ms(&self) -> i64 {
        let fixed = self.fixed_ms.load(Ordering::Relaxed);
        if fixed != 0 {
            return fixed;
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0)
    }

    /// OP11: ids numbered from 1, so a golden's ids are the same on every
    /// run.
    #[cfg(feature = "test-support")]
    pub fn fix_ids(&self) {
        self.next.store(1, Ordering::Relaxed);
    }

    /// OP11: a fixed clock, in Unix milliseconds (not 0).
    #[cfg(feature = "test-support")]
    pub fn fix_time(&self, ms: i64) {
        self.fixed_ms.store(ms, Ordering::Relaxed);
    }
}

/// `ms` as RFC 3339 in UTC, to the second (`2026-11-02T09:14:03Z`), for
/// the times operational records hold (M6-11); before 1970 is written as
/// 1970's first second.
pub fn rfc3339(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Howard Hinnant's civil_from_days, for days since 1970-01-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_rfc_3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(-5), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_790_000_000_000), "2026-09-21T14:13:20Z");
        assert_eq!(rfc3339(1_798_761_599_999), "2026-12-31T23:59:59Z");
    }

    #[test]
    fn ids_are_26_base32_characters() {
        assert_eq!(base32(0), "0".repeat(26));
        assert_eq!(base32(u128::MAX).len(), 26);
        assert!(base32(u128::MAX).starts_with('7'));
        let ids = Ids::default();
        let (a, b) = (ids.id("r"), ids.id("r"));
        assert_ne!(a, b);
        assert_eq!(a.len(), 28);
        assert!(a.starts_with("r_"));
    }
}
