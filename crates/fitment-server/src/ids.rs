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

#[cfg(test)]
mod tests {
    use super::*;

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
