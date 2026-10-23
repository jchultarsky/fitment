//! FH2's nonces: a fresh 128-bit value for every request, so a reply to
//! an earlier request, or another helper's, is never read as this one's.
//! No random-number crate is in the graph (FG1): the seed is the SHA-256
//! of what differs from process to process and helper to helper (std's
//! randomly keyed hasher, the process id, the clock), and each nonce the
//! first half of the SHA-256 of the seed and a counter. A nonce matches
//! replies to requests; it is not a secret.

use std::fmt::Write;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use fitment_core::Sha256;

/// A source of nonces.
#[derive(Debug)]
pub(crate) struct Nonces {
    seed: [u8; 32],
    counter: AtomicU64,
}

impl Nonces {
    pub fn new() -> Self {
        let mut seed = Sha256::new();
        for _ in 0..2 {
            // A RandomState's keys are drawn from the OS once per thread,
            // then stepped; each hasher's output differs.
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u64(0x0066_6974_6d65_6e74);
            seed.update(&hasher.finish().to_le_bytes());
        }
        seed.update(&std::process::id().to_le_bytes());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        seed.update(&now.to_le_bytes());
        Self {
            seed: seed.finish(),
            counter: AtomicU64::new(0),
        }
    }

    /// The next nonce: 32 lower-case hex digits.
    pub fn next(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let mut h = Sha256::new();
        h.update(&self.seed);
        h.update(&n.to_le_bytes());
        h.finish()[..16]
            .iter()
            .fold(String::with_capacity(32), |mut out, b| {
                let _ = write!(out, "{b:02x}");
                out
            })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn nonces_are_fresh_within_and_across_sources() {
        let (a, b) = (Nonces::new(), Nonces::new());
        let mut seen = BTreeSet::new();
        for _ in 0..1000 {
            for nonce in [a.next(), b.next()] {
                assert_eq!(nonce.len(), 32);
                assert!(
                    nonce
                        .bytes()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                );
                assert!(seen.insert(nonce));
            }
        }
    }
}
