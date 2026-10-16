//! `Sha256Hex` (`docs/research/m5-catalog.md`, PH1, DB6): exactly 64
//! characters `[0-9a-f]`, the only value a store path is built from.

use std::fmt;

/// A SHA-256 as 64 lower-case hex digits, checked.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Hex(String);

/// A value that is not 64 lower-case hex digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a SHA-256 of 64 lower-case hex digits: {0:?}")]
pub struct NotHex(pub String);

impl Sha256Hex {
    /// `text`, if it is 64 lower-case hex digits.
    ///
    /// # Errors
    ///
    /// [`NotHex`] for anything else: upper case, another length, a path.
    pub fn parse(text: &str) -> Result<Self, NotHex> {
        if text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(Self(text.to_owned()))
        } else {
            Err(NotHex(text.to_owned()))
        }
    }

    /// The SHA-256 of `bytes`, by core's function.
    pub fn of(bytes: &[u8]) -> Self {
        Self(fitment_core::sha256_hex(bytes))
    }

    /// The 64 digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The fan-out directory's name: the first two digits.
    pub fn fan_out(&self) -> &str {
        self.0.get(..2).unwrap_or_default()
    }

    /// The first `n` digits, for people.
    pub fn short(&self, n: usize) -> &str {
        self.0.get(..n.min(64)).unwrap_or_default()
    }
}

impl fmt::Display for Sha256Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Sha256Hex;

    #[test]
    fn only_lower_case_hex_of_64_digits_is_a_sha256() {
        let good = "ab".repeat(32);
        assert_eq!(Sha256Hex::parse(&good).unwrap().as_str(), good);
        for bad in [
            "AB".repeat(32),
            "ab".repeat(31),
            format!("{}a", "ab".repeat(32)),
            format!("../{}", "a".repeat(61)),
            "g".repeat(64),
            String::new(),
        ] {
            assert!(Sha256Hex::parse(&bad).is_err(), "{bad}");
        }
        let of = Sha256Hex::of(b"");
        assert_eq!(
            of.as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(of.fan_out(), "e3");
    }
}
