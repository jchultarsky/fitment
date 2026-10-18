//! Query strings (docs/research/m6-api.md, VN3, AU3, AU8): each parameter
//! decoded once and checked against the operation's list; an unknown or
//! repeated one is `400 invalid_parameter`, never ignored. Values are
//! never echoed (HX5).

use std::collections::BTreeMap;

use super::error::{ApiError, ErrorCode};

/// The percent-decoding of `text` (`+` is a space, as forms write it), if
/// it is UTF-8 after decoding.
pub fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The parameters of `query`, each named in `allowed`.
///
/// # Errors
///
/// `invalid_parameter` for an unknown, repeated or undecodable one.
pub fn params(query: Option<&str>, allowed: &[&str]) -> Result<BTreeMap<String, String>, ApiError> {
    let mut out = BTreeMap::new();
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(name), Some(value)) = (decode(name), decode(value)) else {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "a query parameter cannot be decoded",
            ));
        };
        if !allowed.contains(&name.as_str()) {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "this route takes no such query parameter",
            )
            .with("allowed", allowed.to_vec()));
        }
        if out.insert(name, value).is_some() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "a query parameter is given twice",
            ));
        }
    }
    Ok(out)
}

/// A page's `limit` (VN3): 100 by default (at most `page_max`), 1 to
/// `page_max` when given.
///
/// # Errors
///
/// `invalid_parameter`.
pub fn limit(params: &BTreeMap<String, String>, page_max: u32) -> Result<u32, ApiError> {
    match params.get("limit") {
        None => Ok(100.min(page_max)),
        Some(text) => text
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=page_max).contains(n))
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("limit is 1 to {page_max}"),
                )
                .with("parameter", "limit")
            }),
    }
}

/// A page's cursor (VN3): opaque, the base64url of the last item's public
/// key; empty for the first page.
///
/// # Errors
///
/// `invalid_parameter` for one this server did not write.
pub fn after(params: &BTreeMap<String, String>) -> Result<String, ApiError> {
    let Some(text) = params.get("after") else {
        return Ok(String::new());
    };
    crate::auth::token::unbase64url(text)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                "after is not a cursor of this list",
            )
            .with("parameter", "after")
        })
}

/// The cursor after `key`.
pub fn cursor(key: &str) -> String {
    crate::auth::token::base64url(key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_decoded_and_checked() {
        let got = params(Some("after=YQ&limit=5"), &["after", "limit"]).unwrap();
        assert_eq!(got["after"], "YQ");
        assert_eq!(after(&got).unwrap(), "a");
        assert_eq!(limit(&got, 1000).unwrap(), 5);
        for bad in ["nope=1", "limit=1&limit=2", "limit=%zz", "after=%ff"] {
            assert!(params(Some(bad), &["after", "limit"]).is_err(), "{bad}");
        }
        let got = params(Some("limit=0"), &["limit"]).unwrap();
        assert!(limit(&got, 1000).is_err());
        let got = params(Some("limit=1001"), &["limit"]).unwrap();
        assert!(limit(&got, 1000).is_err());
        assert_eq!(limit(&BTreeMap::new(), 50).unwrap(), 50);
        assert_eq!(decode("a%20b+c"), Some("a b c".into()));
    }
}
