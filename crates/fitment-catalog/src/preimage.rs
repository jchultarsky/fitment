//! The row preimages and their hashes (`docs/research/m5-catalog.md`, GT2,
//! IX9): the only code that builds or checks a derived row's hash. Each
//! preimage is the row's key and status fields, each as ASCII followed by
//! one `\n`, then the stored bytes. Every field has a fixed alphabet
//! without `\n`, so the encoding is injective, and a row moved to another
//! key, or with a key, version, status or digest column edited, no longer
//! hashes to its own hash.

use crate::hex::Sha256Hex;

/// A `features` row's status (GT1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Accepted by the gate: indexed.
    Accepted,
    /// Refused by the gate: quarantined.
    Refused,
}

impl Status {
    /// As the column holds it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Refused => "refused",
        }
    }

    /// From the column.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "accepted" => Some(Self::Accepted),
            "refused" => Some(Self::Refused),
            _ => None,
        }
    }
}

/// `doc_sha256`: over `<sha256>\n<extractor_version>\n<status>\n<part
/// digest or ->\n<doc>`.
pub fn features(
    sha256: &Sha256Hex,
    extractor_version: u32,
    status: Status,
    part_digest: Option<&Sha256Hex>,
    doc: &[u8],
) -> Sha256Hex {
    let digest = part_digest.map_or("-", Sha256Hex::as_str);
    let head = format!(
        "{sha256}\n{extractor_version}\n{}\n{digest}\n",
        status.as_str()
    );
    Sha256Hex::of(&[head.as_bytes(), doc].concat())
}

/// `tokens_sha256`: over `<sha256>\n<extractor_version>\n<signature
/// version>\n<tokens>`.
pub fn tokens(
    sha256: &Sha256Hex,
    extractor_version: u32,
    signature_version: u32,
    tokens: &[u8],
) -> Sha256Hex {
    let head = format!("{sha256}\n{extractor_version}\n{signature_version}\n");
    Sha256Hex::of(&[head.as_bytes(), tokens].concat())
}

/// `stated_sha256`: over `<sha256>\n<properties_version>\n<truncated: 0
/// or 1>\n<stated>` (PR3).
pub fn properties(
    sha256: &Sha256Hex,
    properties_version: u32,
    truncated: bool,
    stated: &[u8],
) -> Sha256Hex {
    let head = format!("{sha256}\n{properties_version}\n{}\n", u8::from(truncated));
    Sha256Hex::of(&[head.as_bytes(), stated].concat())
}

#[cfg(test)]
mod tests {
    use super::{Sha256Hex, Status, features, properties, tokens};

    /// Every field takes part: changing any one changes the hash.
    #[test]
    fn every_field_is_in_the_preimage() {
        let (x, y) = (Sha256Hex::of(b"x"), Sha256Hex::of(b"y"));
        let base = features(&x, 6, Status::Accepted, Some(&y), b"{}");
        for other in [
            features(&y, 6, Status::Accepted, Some(&y), b"{}"),
            features(&x, 7, Status::Accepted, Some(&y), b"{}"),
            features(&x, 6, Status::Refused, Some(&y), b"{}"),
            features(&x, 6, Status::Accepted, Some(&x), b"{}"),
            features(&x, 6, Status::Accepted, None, b"{}"),
            features(&x, 6, Status::Accepted, Some(&y), b"{ }"),
        ] {
            assert_ne!(other, base);
        }
        let base = tokens(&x, 6, 2, b"{}");
        for other in [
            tokens(&y, 6, 2, b"{}"),
            tokens(&x, 5, 2, b"{}"),
            tokens(&x, 6, 1, b"{}"),
            tokens(&x, 6, 2, b"[]"),
        ] {
            assert_ne!(other, base);
        }
        // Digits do not run into each other: version 62 is not 6 then 2.
        assert_ne!(tokens(&x, 62, 1, b""), tokens(&x, 6, 21, b""));
        let base = properties(&x, 1, false, b"{}");
        for other in [
            properties(&y, 1, false, b"{}"),
            properties(&x, 2, false, b"{}"),
            properties(&x, 1, true, b"{}"),
            properties(&x, 1, false, b"[]"),
        ] {
            assert_ne!(other, base);
        }
        assert_eq!(
            base.as_str(),
            Sha256Hex::of(format!("{x}\n1\n0\n{{}}").as_bytes()).as_str()
        );
    }
}
