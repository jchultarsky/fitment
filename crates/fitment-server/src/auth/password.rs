//! Passwords (docs/research/m6-api.md, AU1, AU2): argon2id with the pepper
//! as argon2's secret and explicit, recorded parameters.
//!
//! Ported from Vectera's `catalog/src/auth.rs` (MIT, © 2024 Julian
//! Chultarsky, the same author): `hash_password`, `verify_password` and
//! the 12-character minimum, with three of the audit's flaws fixed (section
//! 4.11's table): the pepper is argon2's `secret`, not bytes appended to
//! the password (6); the length is counted in characters, 12 to 256, and
//! at most 1,024 bytes, refused before hashing (7); the parameters are
//! explicit (m = 19 MiB, t = 2, p = 1), carried in the PHC string, and a
//! string of other parameters is rehashed at the next login (9). Every call
//! here is blocking: the server runs them on the blocking pool (8, SV3).

use std::fmt;

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use argon2::{Algorithm, Argon2, Params, Version};

/// argon2's memory, in KiB: 19 MiB (OWASP's minimum for argon2id).
pub const M_COST_KIB: u32 = 19 * 1024;

/// argon2's passes.
pub const T_COST: u32 = 2;

/// argon2's lanes.
pub const P_COST: u32 = 1;

/// The fewest characters a password has (Vectera's `MIN_PASSWORD_LEN`).
pub const MIN_CHARS: usize = 12;

/// The most characters (Unicode scalar values) a password has.
pub const MAX_CHARS: usize = 256;

/// The most bytes a password has.
pub const MAX_BYTES: usize = 1024;

/// The salt's length in bytes (argon2's recommended 16).
const SALT_BYTES: usize = 16;

/// The pepper (AU1): a server-wide secret given to argon2 as its key, read
/// from `auth.pepper_file`; none when the file is not configured. Its
/// `Debug` never shows it (OP1).
#[derive(Clone, Default)]
pub struct Pepper(Option<Vec<u8>>);

impl Pepper {
    /// No pepper: argon2 without a secret.
    pub fn none() -> Self {
        Self(None)
    }

    /// The pepper `bytes`.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Some(bytes))
    }

    /// True if a pepper is set.
    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }
}

impl fmt::Debug for Pepper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_set() {
            "Pepper(set)"
        } else {
            "Pepper(none)"
        })
    }
}

/// Why a password is refused before any hashing (AU1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    /// Fewer than 12 characters.
    #[error("a password has at least 12 characters")]
    TooShort,
    /// More than 256 characters or 1,024 bytes.
    #[error("a password has at most 256 characters and 1,024 bytes")]
    TooLong,
}

/// AU1's bounds, counted in Unicode scalar values (characters), not bytes
/// (audit flaw 7), the password compared as given (no normalization).
///
/// # Errors
///
/// [`PasswordError`].
pub fn check(password: &str) -> Result<(), PasswordError> {
    if password.len() > MAX_BYTES {
        return Err(PasswordError::TooLong);
    }
    match password.chars().count() {
        n if n < MIN_CHARS => Err(PasswordError::TooShort),
        n if n > MAX_CHARS => Err(PasswordError::TooLong),
        _ => Ok(()),
    }
}

/// AU1's parameters.
fn params() -> Params {
    // Within argon2's bounds: this cannot fail.
    Params::new(M_COST_KIB, T_COST, P_COST, None).unwrap_or(Params::DEFAULT)
}

/// argon2id, version 0x13, AU1's parameters, keyed by the pepper (fix 6:
/// `Argon2::new_with_secret`, keyed hashing as the algorithm defines it).
fn argon2(pepper: &Pepper) -> Argon2<'_> {
    match &pepper.0 {
        Some(secret) => {
            Argon2::new_with_secret(secret, Algorithm::Argon2id, Version::V0x13, params())
                .unwrap_or_else(|_| Argon2::new(Algorithm::Argon2id, Version::V0x13, params()))
        }
        None => Argon2::new(Algorithm::Argon2id, Version::V0x13, params()),
    }
}

/// Hashing failed: only a broken random source or parameters argon2
/// refuses could cause it.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("the password could not be hashed")]
pub struct HashError;

/// The PHC string of `password` (`$argon2id$v=19$m=19456,t=2,p=1$…`), a
/// fresh 16-byte salt from `getrandom`.
///
/// # Errors
///
/// [`HashError`].
pub fn hash(password: &str, pepper: &Pepper) -> Result<String, HashError> {
    let mut salt = [0_u8; SALT_BYTES];
    getrandom::fill(&mut salt).map_err(|_| HashError)?;
    argon2(pepper)
        .hash_password_with_salt(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| HashError)
}

/// What a verification found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verified {
    /// The password matches; `rehash` when the PHC string's parameters are
    /// not this build's (fix 9).
    Match {
        /// The string should be replaced by one of AU1's parameters.
        rehash: bool,
    },
    /// It does not, or the string cannot be read.
    NoMatch,
}

/// Verifies `password` against `phc` under `pepper`, with the string's own
/// parameters (so an older string still verifies). argon2 compares the
/// outputs in constant time.
pub fn verify(password: &str, pepper: &Pepper, phc: &str) -> Verified {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return Verified::NoMatch;
    };
    match argon2(pepper).verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Verified::Match {
            rehash: !is_current(&parsed),
        },
        Err(_) => Verified::NoMatch,
    }
}

/// True if `parsed` is argon2id, version 0x13, with AU1's parameters.
fn is_current(parsed: &PasswordHash) -> bool {
    let ours = params();
    parsed.algorithm == Algorithm::Argon2id.ident()
        && parsed.version == Some(Version::V0x13.into())
        && Params::try_from(parsed).is_ok_and(|p| {
            p.m_cost() == ours.m_cost()
                && p.t_cost() == ours.t_cost()
                && p.p_cost() == ours.p_cost()
        })
}

/// AU2's stand-in: the PHC string of a random password under AU1's
/// parameters and the pepper, made at startup, verified for an unknown,
/// disabled or password-less user so a login costs one verification
/// whatever the username.
///
/// # Errors
///
/// [`HashError`].
pub fn dummy(pepper: &Pepper) -> Result<String, HashError> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).map_err(|_| HashError)?;
    let password = super::token::base64url(&bytes);
    hash(&password, pepper)
}

/// A PHC string of `password` under other parameters (m = 64 KiB, t = 1):
/// what a string of an older policy looks like, for fix 9's test.
///
/// # Errors
///
/// [`HashError`].
#[cfg(feature = "test-support")]
pub fn hash_with_old_parameters(password: &str, pepper: &Pepper) -> Result<String, HashError> {
    let old = Params::new(64, 1, 1, None).map_err(|_| HashError)?;
    let argon2 = match &pepper.0 {
        Some(secret) => Argon2::new_with_secret(secret, Algorithm::Argon2id, Version::V0x13, old)
            .map_err(|_| HashError)?,
        None => Argon2::new(Algorithm::Argon2id, Version::V0x13, old),
    };
    argon2
        .hash_password_with_salt(password.as_bytes(), b"an-older-salt-16")
        .map(|h| h.to_string())
        .map_err(|_| HashError)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectera's `password_round_trip`, ported: the right password and
    /// pepper verify; a wrong password or another pepper do not.
    #[test]
    fn password_round_trip() {
        let pepper = Pepper::new(b"unit-test-pepper-0123".to_vec());
        let phc = hash("correct horse battery", &pepper).unwrap();
        assert_eq!(
            verify("correct horse battery", &pepper, &phc),
            Verified::Match { rehash: false }
        );
        assert_eq!(verify("wrong password!", &pepper, &phc), Verified::NoMatch);
        let other = Pepper::new(b"another-pepper-456789".to_vec());
        assert_eq!(
            verify("correct horse battery", &other, &phc),
            Verified::NoMatch
        );
        assert_eq!(
            verify("correct horse battery", &Pepper::none(), &phc),
            Verified::NoMatch
        );
        assert_eq!(verify("x", &pepper, "not a PHC string"), Verified::NoMatch);
    }

    /// AU1, fix 6: the pepper is argon2's secret. Vectera's form, the
    /// pepper appended to the password and hashed without a secret, does
    /// not verify against our string, and ours is argon2's keyed hash.
    #[test]
    fn the_pepper_is_argon2s_secret() {
        let pepper = Pepper::new(b"PEPPER-unit-0123456789".to_vec());
        let phc = hash("correct horse battery", &pepper).unwrap();
        let appended = format!("correct horse battery{}", "PEPPER-unit-0123456789");
        assert_eq!(verify(&appended, &Pepper::none(), &phc), Verified::NoMatch);
        let parsed = PasswordHash::new(&phc).unwrap();
        let salt = parsed.salt.unwrap();
        let keyed = Argon2::new_with_secret(
            b"PEPPER-unit-0123456789",
            Algorithm::Argon2id,
            Version::V0x13,
            params(),
        )
        .unwrap()
        .hash_password_with_salt(b"correct horse battery", salt.as_ref())
        .unwrap()
        .to_string();
        assert_eq!(keyed, phc);
    }

    /// AU1, fix 9: the parameters are explicit and recorded in the string;
    /// a string of other parameters verifies and asks for a rehash.
    #[test]
    fn the_parameters_are_recorded() {
        let phc = hash("correct horse battery", &Pepper::none()).unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        let old = Params::new(64, 1, 1, None).unwrap();
        let older = Argon2::new(Algorithm::Argon2id, Version::V0x13, old)
            .hash_password_with_salt(b"correct horse battery", b"an-older-salt-16")
            .unwrap()
            .to_string();
        assert_eq!(
            verify("correct horse battery", &Pepper::none(), &older),
            Verified::Match { rehash: true }
        );
    }

    /// AU1, fix 7: 12 to 256 characters, at most 1,024 bytes, counted in
    /// characters: twelve two-byte characters pass, eleven fail, as six
    /// would have passed Vectera's byte count.
    #[test]
    fn lengths_are_characters_with_an_upper_bound() {
        assert_eq!(check(&"a".repeat(11)), Err(PasswordError::TooShort));
        assert_eq!(check(&"a".repeat(12)), Ok(()));
        assert_eq!(check(&"é".repeat(6)), Err(PasswordError::TooShort));
        assert_eq!(check(&"é".repeat(12)), Ok(()));
        assert_eq!(check(&"a".repeat(256)), Ok(()));
        assert_eq!(check(&"a".repeat(257)), Err(PasswordError::TooLong));
        assert_eq!(check(&"\u{1F600}".repeat(256)), Ok(()), "1,024 bytes");
        assert_eq!(check(&"a".repeat(1025)), Err(PasswordError::TooLong));
    }

    #[test]
    fn the_pepper_is_never_shown() {
        let pepper = Pepper::new(b"PEPPER-secret".to_vec());
        assert_eq!(format!("{pepper:?}"), "Pepper(set)");
    }
}
