//! Ids and references (`docs/research/m5-catalog.md`, ID1 to ID9).

use std::fmt;

/// The longest id (ID1).
const MAX_ID: usize = 64;

/// An entry's id (ID1): `[A-Za-z0-9][A-Za-z0-9._+-]{0,63}`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

/// Text that is not an id, or not a reference.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BadReference {
    /// Not an id (ID1).
    #[error(
        "{0:?} is not an id: ids are 1 to 64 of A-Z a-z 0-9 . _ + -, starting with a letter or digit"
    )]
    Id(String),
    /// Something after `@` that is not `r` and a revision (ID9).
    #[error("{0:?} is not a revision reference: write ID@rN, N from 1 with no leading zero")]
    Revision(String),
    /// A `sha256:` reference with too few or non-hex digits (ID7).
    #[error("{0:?} is not a content reference: write sha256: and 12 to 64 hex digits")]
    Sha256(String),
}

fn in_alphabet(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')
}

impl Id {
    /// `text`, if it is an id.
    ///
    /// # Errors
    ///
    /// [`BadReference::Id`] otherwise: an id the user typed is never mapped.
    pub fn parse(text: &str) -> Result<Self, BadReference> {
        let bytes = text.as_bytes();
        let ok = (1..=MAX_ID).contains(&bytes.len())
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.iter().all(|b| in_alphabet(*b));
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(BadReference::Id(text.to_owned()))
        }
    }

    /// The id as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id under ASCII case folding (ID2): what uniqueness and lookup
    /// compare.
    pub fn fold(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A file name without its `.stp`, `.step` or `.p21` extension, in any case.
pub fn stem(file_name: &str) -> &str {
    for ext in [".stp", ".step", ".p21"] {
        let Some(at) = file_name.len().checked_sub(ext.len()) else {
            continue;
        };
        if let (Some(stem), Some(tail)) = (file_name.get(..at), file_name.get(at..)) {
            if tail.eq_ignore_ascii_case(ext) {
                return stem;
            }
        }
    }
    file_name
}

/// ID3's steps 2 and 3: each run of characters outside the alphabet as one
/// `_`, leading `.`, `_`, `+` and `-` dropped, cut at 64.
fn map(stem: &str) -> Option<Id> {
    let mut out = String::new();
    let mut outside = false;
    for c in stem.chars() {
        if u8::try_from(c).is_ok_and(in_alphabet) {
            out.push(c);
            outside = false;
        } else if !outside {
            out.push('_');
            outside = true;
        }
    }
    let out = out.trim_start_matches(['.', '_', '+', '-']);
    Id::parse(out.get(..out.len().min(MAX_ID)).unwrap_or(out)).ok()
}

/// ID3: the id a file of `file_name` gets without `--id`, or `None` when
/// nothing of it is left ("no valid id: give --id").
pub fn default_id(file_name: &str) -> Option<Id> {
    map(stem(file_name))
}

/// ID4: whether `file_name`'s stem is already an id, so it is ingested in
/// the first group.
pub fn is_exact(file_name: &str) -> bool {
    Id::parse(stem(file_name)).is_ok()
}

/// What a command names an entry by (ID7, ID9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// An id: its live entry (or, for `get`, its latest tombstone).
    Id(Id),
    /// `ID@rN`: one revision of an id.
    Revision(Id, u32),
    /// `sha256:HEX`: the live entry whose content starts with these
    /// lower-cased digits, 12 to 64 of them.
    Sha256(String),
}

impl Reference {
    /// Reads a reference.
    ///
    /// # Errors
    ///
    /// [`BadReference`] for anything else; text after an `@` that is not a
    /// strict revision is never read as a bare id (ID9).
    pub fn parse(text: &str) -> Result<Self, BadReference> {
        if let Some(digits) = text.strip_prefix("sha256:") {
            let ok =
                (12..=64).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_hexdigit());
            return if ok {
                Ok(Self::Sha256(digits.to_ascii_lowercase()))
            } else {
                Err(BadReference::Sha256(text.to_owned()))
            };
        }
        let Some((id, revision)) = text.split_once('@') else {
            return Id::parse(text).map(Self::Id);
        };
        let id = Id::parse(id)?;
        let n = revision
            .strip_prefix('r')
            .filter(|n| {
                n.bytes().all(|b| b.is_ascii_digit()) && n.bytes().next().is_some_and(|b| b != b'0')
            })
            .and_then(|n| n.parse::<u32>().ok())
            .ok_or_else(|| BadReference::Revision(text.to_owned()))?;
        Ok(Self::Revision(id, n))
    }
}

#[cfg(test)]
mod tests {
    use super::{BadReference, Id, Reference, default_id, is_exact, map};

    #[test]
    fn default_ids_follow_id3() {
        let id = |name: &str| default_id(name).map(|i| i.as_str().to_owned());
        assert_eq!(id("a b.stp").as_deref(), Some("a_b"));
        assert_eq!(id("m_bracket.STEP").as_deref(), Some("m_bracket"));
        assert_eq!(id("part.p21").as_deref(), Some("part"));
        assert_eq!(
            id("evil\u{1b}[31mred\u{202e}.stp").as_deref(),
            Some("evil_31mred_")
        );
        assert_eq!(
            id("copy of m_bracket.stp").as_deref(),
            Some("copy_of_m_bracket")
        );
        assert_eq!(id("..-_x.stp").as_deref(), Some("x"));
        assert_eq!(id("é.stp"), None);
        assert_eq!(id(".stp"), None);
        assert_eq!(id(&format!("{}.stp", "a".repeat(80))).unwrap().len(), 64);
        assert!(is_exact("m_bracket.stp"));
        assert!(!is_exact("a b.stp"));
    }

    /// CT17, as a property over many strings: the mapping gives an id or
    /// nothing, and an id maps to itself.
    #[test]
    fn the_mapping_gives_an_id_or_none_and_keeps_ids() {
        let alphabet: Vec<char> = "aZ09._+- /\\:\u{1b}\u{202e}é\n@%'".chars().collect();
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let len = usize::try_from(next() % 80).unwrap();
            let text: String = (0..len)
                .map(|_| alphabet[usize::try_from(next()).unwrap() % alphabet.len()])
                .collect();
            if let Some(id) = map(&text) {
                assert_eq!(Id::parse(id.as_str()).as_ref(), Ok(&id), "{text:?}");
            }
            if let Ok(id) = Id::parse(&text) {
                assert_eq!(map(&text), Some(id), "{text:?}");
            }
        }
    }

    #[test]
    fn revision_references_are_strict() {
        let id = Id::parse("BRKT").unwrap();
        assert_eq!(
            Reference::parse("BRKT@r1"),
            Ok(Reference::Revision(id.clone(), 1))
        );
        assert_eq!(
            Reference::parse("BRKT@r12"),
            Ok(Reference::Revision(id.clone(), 12))
        );
        assert_eq!(Reference::parse("BRKT"), Ok(Reference::Id(id)));
        for bad in [
            "BRKT@R1", "BRKT@r01", "BRKT@r0", "BRKT@1", "BRKT@r", "BRKT@r1x", "BRKT@r-1",
        ] {
            assert!(
                matches!(Reference::parse(bad), Err(BadReference::Revision(_))),
                "{bad}"
            );
        }
        assert!(matches!(
            Reference::parse("a b@r1"),
            Err(BadReference::Id(_))
        ));
        assert_eq!(
            Reference::parse("sha256:1B77EA9724E6"),
            Ok(Reference::Sha256("1b77ea9724e6".into()))
        );
        assert!(Reference::parse("sha256:1b77ea97").is_err());
        assert!(Reference::parse("sha256:1b77ea9724zz").is_err());
        for hostile in ["a'b", "a%", "a;", "-a", ".a", ""] {
            assert!(Reference::parse(hostile).is_err(), "{hostile}");
        }
        assert!(Reference::parse("a_b").is_ok());
    }
}
