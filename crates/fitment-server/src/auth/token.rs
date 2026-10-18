//! Tokens (docs/research/m6-api.md, AU3, M6-18): `fitment_pat_` and the
//! base64url of 32 random bytes, stored only as the SHA-256 of the whole
//! string and shown once, at creation.
//!
//! Ported from Vectera's `catalog/src/auth.rs` (MIT, © 2024 Julian
//! Chultarsky, the same author): `generate_token`, `hash_token` and the
//! prefixed personal access token (`vctp_` there), with the SHA-256 from
//! fitment-core (M6-16) and the bearer header the only way in (AU3).

use std::fmt;

/// The prefix that lets secret scanners and logs recognize a token (AU3).
pub const PREFIX: &str = "fitment_pat_";

/// The characters after the prefix: base64url of 32 bytes, no padding.
pub const BODY_CHARS: usize = 43;

/// A token's plaintext (AU3): only ever in the creating response, never
/// logged or stored; its `Debug` and `Display` show the prefix and its
/// hint alone.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// A token from 32 random bytes (Vectera's `generate_token`).
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(format!("{PREFIX}{}", base64url(bytes)))
    }

    /// The token presented in a bearer header, if it is one's shape: the
    /// prefix and 43 base64url characters.
    pub fn parse(text: &str) -> Option<Self> {
        let body = text.strip_prefix(PREFIX)?;
        let ok = body.len() == BODY_CHARS
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        ok.then(|| Self(text.to_owned()))
    }

    /// The plaintext, for the one response that carries it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The first four characters after the prefix, stored and shown so its
    /// owner can tell tokens apart (`token.hint`).
    pub fn hint(&self) -> String {
        self.0[PREFIX.len()..].chars().take(4).collect()
    }

    /// `secret_sha256`: core's SHA-256 of the whole string, lower-case hex
    /// (Vectera's `hash_token`). A lookup by it needs no constant-time
    /// comparison: it is a preimage-resistant hash of 256 random bits.
    pub fn sha256(&self) -> String {
        fitment_core::sha256_hex(self.0.as_bytes())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}…{}", self.hint())
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// base64url without padding (RFC 4648, section 5).
pub fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            let index = (n >> (18 - 6 * i)) & 0x3f;
            out.push(char::from(TABLE[usize::try_from(index).unwrap_or(0)]));
        }
    }
    out
}

/// The text base64url without padding encodes, if it is one.
pub fn unbase64url(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0_u32;
    let mut count = 0;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for b in text.bytes() {
        let value = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from((bits >> count) & 0xff).ok()?);
            bits &= (1 << count) - 1;
        }
    }
    (count < 6 && bits & ((1 << count) - 1) == 0).then_some(out)
}

/// The bearer token of an `Authorization` header's value (AU3): the
/// scheme `Bearer` (any case), one space, the token.
pub fn bearer(value: &str) -> Option<Secret> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Secret::parse(token)
}

/// True if a request's query names a token (AU3): one there would reach
/// logs and proxies, so the request is refused whatever else it holds.
pub fn in_query(query: &str) -> bool {
    query.split('&').any(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded = crate::http::query::decode(value).unwrap_or_default();
        let name = crate::http::query::decode(name).unwrap_or_default();
        name.eq_ignore_ascii_case("access_token")
            || decoded.to_ascii_lowercase().contains(PREFIX)
            || name.to_ascii_lowercase().contains(PREFIX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AU3: a token is the prefix and 43 base64url characters; its hash is
    /// core's SHA-256 of the whole string; its `Debug` hides it.
    #[test]
    fn tokens_are_prefixed_hashed_and_hidden() {
        let secret = Secret::from_bytes(&[7; 32]);
        assert!(secret.expose().starts_with(PREFIX));
        assert_eq!(secret.expose().len(), PREFIX.len() + BODY_CHARS);
        assert_eq!(Secret::parse(secret.expose()), Some(secret.clone()));
        assert_eq!(
            secret.sha256(),
            fitment_core::sha256_hex(secret.expose().as_bytes())
        );
        let shown = format!("{secret:?} {secret}");
        assert!(!shown.contains(&secret.expose()[PREFIX.len()..]), "{shown}");
        assert_eq!(secret.hint().len(), 4);
        assert_ne!(Secret::from_bytes(&[8; 32]).sha256(), secret.sha256());
    }

    #[test]
    fn bearer_headers_are_read_strictly() {
        let secret = Secret::from_bytes(&[1; 32]);
        let token = secret.expose().to_owned();
        assert_eq!(bearer(&format!("Bearer {token}")), Some(secret.clone()));
        assert_eq!(bearer(&format!("bearer {token}")), Some(secret));
        for bad in [
            token.clone(),
            format!("Basic {token}"),
            format!("Bearer  {token}"),
            format!("Bearer {token}x"),
            format!("Bearer {}", &token[..token.len() - 1]),
            "Bearer fitment_pat_".to_owned(),
            format!("Bearer vctp_{}", &token[PREFIX.len()..]),
        ] {
            assert_eq!(bearer(&bad), None, "{bad}");
        }
    }

    #[test]
    fn base64url_round_trips() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        for n in 0..40_u8 {
            let bytes: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(unbase64url(&base64url(&bytes)), Some(bytes));
        }
        assert_eq!(unbase64url("a"), None);
        assert_eq!(unbase64url("Zh"), None, "non-zero trailing bits");
        assert_eq!(unbase64url("Z="), None);
    }

    #[test]
    fn a_token_in_a_query_is_found() {
        let token = Secret::from_bytes(&[2; 32]);
        assert!(in_query(&format!("token={}", token.expose())));
        assert!(in_query("access_token=abc"));
        assert!(in_query("x=fitment%5Fpat%5Fabc"));
        assert!(!in_query("after=abc&limit=10"));
    }
}
