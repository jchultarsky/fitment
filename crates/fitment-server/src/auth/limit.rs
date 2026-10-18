//! Rate limits (docs/research/m6-api.md, AU7, M6-30, M6-61): in-memory
//! token buckets, reset by a restart. Requests are counted per user, over
//! all their tokens; logins per client address, then failures per (folded
//! username, address) for any username, existing or not, in at most
//! `auth.login_buckets` buckets, the least recently used evicted.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

/// A bucket's shape: `burst` tokens at most, refilled at `count` per
/// `window_ms`. Levels are kept in millionths of a token, so the arithmetic
/// is exact integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    /// The most tokens it holds.
    pub burst: u64,
    /// Tokens refilled per window.
    pub count: u64,
    /// The window, in milliseconds.
    pub window_ms: u64,
}

/// A token, in a level's units.
const ONE: u128 = 1_000_000;

impl Rate {
    /// `count` tokens per `window_ms`, all of them at once at most.
    pub fn per(count: u32, window_ms: u64) -> Self {
        Self {
            burst: u64::from(count.max(1)),
            count: u64::from(count.max(1)),
            window_ms: window_ms.max(1),
        }
    }

    /// `burst` tokens at most, refilled at `per_second` a second.
    pub fn burst_and_rate(burst: u32, per_second: u32) -> Self {
        Self {
            burst: u64::from(burst.max(1)),
            count: u64::from(per_second.max(1)),
            window_ms: 1000,
        }
    }

    fn full(self) -> u128 {
        u128::from(self.burst) * ONE
    }
}

/// One bucket: its level when last seen, and when.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    level: u128,
    at_ms: u64,
}

impl Bucket {
    fn full(rate: Rate, now_ms: u64) -> Self {
        Self {
            level: rate.full(),
            at_ms: now_ms,
        }
    }

    fn refill(&mut self, rate: Rate, now_ms: u64) {
        let elapsed = u128::from(now_ms.saturating_sub(self.at_ms));
        let gained = elapsed * u128::from(rate.count) * ONE / u128::from(rate.window_ms);
        self.level = (self.level + gained).min(rate.full());
        self.at_ms = now_ms.max(self.at_ms);
    }

    /// The milliseconds until one token is there; 0 if it is.
    fn wait_ms(&self, rate: Rate) -> u64 {
        if self.level >= ONE {
            return 0;
        }
        let missing = (ONE - self.level) * u128::from(rate.window_ms);
        let per_ms = u128::from(rate.count) * ONE;
        u64::try_from(missing.div_ceil(per_ms))
            .unwrap_or(u64::MAX)
            .max(1)
    }

    /// Takes one token, or answers how long until one is there.
    fn take(&mut self, rate: Rate, now_ms: u64) -> Result<(), u64> {
        self.refill(rate, now_ms);
        match self.wait_ms(rate) {
            0 => {
                self.level -= ONE;
                Ok(())
            }
            wait => Err(wait),
        }
    }
}

/// Buckets by key, at most `cap` of them, the least recently used evicted
/// when a new one would pass the cap.
#[derive(Debug)]
struct Lru<K> {
    buckets: HashMap<K, (Bucket, u64)>,
    order: BTreeMap<u64, K>,
    seq: u64,
    cap: usize,
}

impl<K: Hash + Eq + Clone> Lru<K> {
    fn new(cap: usize) -> Self {
        Self {
            buckets: HashMap::new(),
            order: BTreeMap::new(),
            seq: 0,
            cap: cap.max(1),
        }
    }

    /// The bucket of `key`, made full if new, now the most recently used.
    fn get(&mut self, key: &K, rate: Rate, now_ms: u64) -> &mut Bucket {
        self.seq += 1;
        let seq = self.seq;
        if let Some((_, old)) = self.buckets.get(key) {
            self.order.remove(old);
        } else {
            while self.buckets.len() >= self.cap {
                let Some((_, oldest)) = self.order.pop_first() else {
                    break;
                };
                self.buckets.remove(&oldest);
            }
        }
        self.order.insert(seq, key.clone());
        let entry = self
            .buckets
            .entry(key.clone())
            .or_insert((Bucket::full(rate, now_ms), seq));
        entry.1 = seq;
        &mut entry.0
    }
}

/// A login bucket's key: a client address, or a folded username's SHA-256
/// (a bounded key for any string) and the address.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LoginKey {
    Address(IpAddr),
    Pair([u8; 32], IpAddr),
}

/// AU7's login limits.
#[derive(Debug)]
pub struct LoginLimits {
    buckets: Mutex<Lru<LoginKey>>,
    attempts: Rate,
    failures: Rate,
}

impl LoginLimits {
    /// `attempts` per address, `failures` per (username, address), in at
    /// most `cap` buckets.
    pub fn new(attempts: Rate, failures: Rate, cap: usize) -> Self {
        Self {
            buckets: Mutex::new(Lru::new(cap)),
            attempts,
            failures,
        }
    }

    /// A login attempt (AU7), under the buckets' lock: one of the
    /// address's attempts, checked first, then one of the pair's failures,
    /// both taken now, so concurrent wrong guesses cannot all pass a peek
    /// before any is charged (review). The [`Attempt`] gives the pair's
    /// token back when dropped, unless [`Attempt::failed`] says the
    /// credentials were wrong: a success, or an exit that never judged
    /// them (a full cap, no argon2 slot, a storage error), costs no
    /// failure. An IPv6 client is keyed by its /64, which one host
    /// usually holds whole.
    ///
    /// # Errors
    ///
    /// The milliseconds until the attempt would be admitted.
    pub fn admit(&self, user: &[u8; 32], address: IpAddr, now_ms: u64) -> Result<Attempt<'_>, u64> {
        let address = network(address);
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        buckets
            .get(&LoginKey::Address(address), self.attempts, now_ms)
            .take(self.attempts, now_ms)?;
        buckets
            .get(&LoginKey::Pair(*user, address), self.failures, now_ms)
            .take(self.failures, now_ms)?;
        Ok(Attempt {
            limits: self,
            user: *user,
            address,
            charged: false,
        })
    }

    /// Gives back the pair's token an admitted attempt took, capped at
    /// full; nothing if the bucket was evicted meanwhile (a new one is
    /// full).
    fn refund(&self, user: &[u8; 32], address: IpAddr) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let full = self.failures.full();
        if let Some((bucket, _)) = buckets.buckets.get_mut(&LoginKey::Pair(*user, address)) {
            bucket.level = (bucket.level + ONE).min(full);
        }
    }

    /// The buckets kept (VT24's count).
    pub fn len(&self) -> usize {
        self.buckets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .buckets
            .len()
    }

    /// True if none is kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An admitted login attempt (see [`LoginLimits::admit`]): dropped, it
/// refunds the pair's token; [`Attempt::failed`] keeps it charged.
#[derive(Debug)]
#[must_use = "an attempt is charged only by `failed`; dropping it refunds the pair's token"]
pub struct Attempt<'a> {
    limits: &'a LoginLimits,
    user: [u8; 32],
    address: IpAddr,
    charged: bool,
}

impl Attempt<'_> {
    /// The credentials were wrong: the pair's token stays taken.
    pub fn failed(mut self) {
        self.charged = true;
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if !self.charged {
            self.limits.refund(&self.user, self.address);
        }
    }
}

/// The key of `address` in the login buckets (AU7): an IPv4 address as it
/// is, an IPv6 one by its /64.
pub fn network(address: IpAddr) -> IpAddr {
    match address.to_canonical() {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// AU7's request limit, per user over all their tokens (M6-61).
#[derive(Debug)]
pub struct RequestLimits {
    buckets: Mutex<HashMap<i64, Bucket>>,
    rate: Rate,
}

/// Past this many users' buckets, full ones are dropped: a full bucket is
/// the one a new user gets.
const PRUNE_AT: usize = 4096;

impl RequestLimits {
    /// `rate` per user.
    pub fn new(rate: Rate) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            rate,
        }
    }

    /// One request of `user`.
    ///
    /// # Errors
    ///
    /// The milliseconds until it would be admitted.
    pub fn take(&self, user: i64, now_ms: u64) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() >= PRUNE_AT {
            let rate = self.rate;
            buckets.retain(|_, b| {
                b.refill(rate, now_ms);
                b.level < rate.full()
            });
        }
        buckets
            .entry(user)
            .or_insert_with(|| Bucket::full(self.rate, now_ms))
            .take(self.rate, now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bucket gives its burst, then one per period.
    #[test]
    fn a_bucket_gives_its_burst_then_its_rate() {
        let rate = Rate::per(10, 1000);
        let limits = RequestLimits::new(rate);
        for _ in 0..10 {
            assert_eq!(limits.take(1, 0), Ok(()));
        }
        assert_eq!(limits.take(1, 0), Err(100));
        assert_eq!(limits.take(2, 0), Ok(()), "another user's bucket");
        assert_eq!(limits.take(1, 99), Err(1));
        assert_eq!(limits.take(1, 100), Ok(()));
    }

    /// AU7 (review): an attempt takes the pair's token at admission; a
    /// dropped attempt refunds it, a failed one keeps it; an IPv6 /64 is
    /// one client.
    #[test]
    fn an_attempt_is_charged_up_front_and_refunded_unless_it_failed() {
        let limits = LoginLimits::new(Rate::per(100, 60_000), Rate::per(2, 60_000), 100);
        let user = [1; 32];
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let first = limits.admit(&user, a, 0).unwrap();
        let second = limits.admit(&user, b, 0).unwrap();
        assert!(
            limits.admit(&user, a, 0).is_err(),
            "two held at once, same /64"
        );
        drop(first);
        second.failed();
        limits.admit(&user, a, 0).unwrap().failed();
        assert!(limits.admit(&user, b, 0).is_err(), "both charged");
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert!(limits.admit(&user, other, 0).is_ok(), "another /64");
        assert_eq!(
            network("10.1.2.3".parse().unwrap()),
            "10.1.2.3".parse::<IpAddr>().unwrap()
        );
    }

    /// The map keeps at most its cap, the least recently used evicted; a
    /// bucket touched survives.
    #[test]
    fn the_least_recently_used_bucket_goes() {
        let mut lru = Lru::new(3);
        let rate = Rate::per(1, 1000);
        for key in 0..3 {
            let _ = lru.get(&key, rate, 0).take(rate, 0);
        }
        let _ = lru.get(&0, rate, 0);
        let _ = lru.get(&3, rate, 0);
        assert_eq!(lru.buckets.len(), 3);
        assert!(lru.buckets.contains_key(&0) && !lru.buckets.contains_key(&1));
        assert_eq!(lru.order.len(), 3);
    }
}
