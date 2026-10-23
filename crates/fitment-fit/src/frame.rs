//! FH1's frames: the four bytes `FFIT`, a 4-byte big-endian header length,
//! the header (UTF-8 JSON, one object), then the payloads the header
//! announces in `"payloads": [n₁, n₂, …]`, each raw. Every cap is checked
//! before anything is allocated for it, and a buffer grows only with the
//! bytes that actually arrive, so a frame that announces more than it
//! sends costs what it sent.

use std::io::{self, Read, Write};

/// The bytes every frame starts with.
pub const MAGIC: [u8; 4] = *b"FFIT";

/// The largest header, in bytes (FH1).
pub const MAX_HEADER_BYTES: u64 = 16 << 20;

/// The largest payload, in bytes (FH1).
pub const MAX_PAYLOAD_BYTES: u64 = 1 << 30;

/// The most payloads one frame carries (FH1).
pub const MAX_PAYLOADS: usize = 4;

/// What one direction of the channel allows (FH1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Caps {
    /// The largest header, in bytes.
    pub header: u64,
    /// The largest payload, in bytes.
    pub payload: u64,
    /// The most payloads.
    pub payloads: usize,
}

impl Caps {
    /// A request's: a header of 16 MiB, four payloads of 1 GiB each.
    pub const REQUEST: Self = Self {
        header: MAX_HEADER_BYTES,
        payload: MAX_PAYLOAD_BYTES,
        payloads: MAX_PAYLOADS,
    };

    /// A reply's: a header of 16 MiB and no payload.
    pub const REPLY: Self = Self {
        header: MAX_HEADER_BYTES,
        payload: 0,
        payloads: 0,
    };
}

/// One frame: its header's bytes and its payloads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Frame {
    /// The header, a JSON object.
    pub header: Vec<u8>,
    /// The payloads, in the header's order.
    pub payloads: Vec<Vec<u8>>,
}

/// Why no frame could be read (FH1): every one is a protocol error but
/// [`FrameError::Closed`], which is the channel's end between frames.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The channel ended between frames.
    #[error("the channel ended")]
    Closed,
    /// The channel ended inside a frame.
    #[error("the channel ended inside a frame")]
    Truncated,
    /// The frame does not start with `FFIT`.
    #[error("a frame does not start with FFIT")]
    Magic,
    /// The header is longer than its cap.
    #[error("a frame announces a header of {0} bytes, over its cap")]
    HeaderTooLarge(u64),
    /// The header is not one JSON object announcing its payloads.
    #[error("a frame's header is not one JSON object announcing its payloads: {0}")]
    Header(String),
    /// More payloads than the cap.
    #[error("a frame announces {0} payloads, over its cap")]
    TooManyPayloads(usize),
    /// A payload longer than its cap.
    #[error("a frame announces a payload of {0} bytes, over its cap")]
    PayloadTooLarge(u64),
    /// Reading failed.
    #[error("reading a frame failed: {0}")]
    Io(String),
}

impl FrameError {
    /// True if the channel ended, between frames or inside one: the
    /// process may have exited, and how it ended decides (FV8).
    pub fn ended(&self) -> bool {
        matches!(self, Self::Closed | Self::Truncated)
    }
}

/// The header's own announcement of its payloads; every other key is the
/// operation's, read by the protocol's strict decoders (FH5).
#[derive(serde::Deserialize)]
struct Announce {
    #[serde(default)]
    payloads: Vec<u64>,
}

/// Reads one frame from `from` under `caps`.
///
/// # Errors
///
/// [`FrameError`]: the channel's end, or a frame that breaks FH1.
pub fn read_frame(from: &mut impl Read, caps: Caps) -> Result<Frame, FrameError> {
    let mut start = [0u8; 8];
    match fill(from, &mut start)? {
        0 => return Err(FrameError::Closed),
        8 => {}
        _ => return Err(FrameError::Truncated),
    }
    if start[..4] != MAGIC {
        return Err(FrameError::Magic);
    }
    let length = u64::from(u32::from_be_bytes([start[4], start[5], start[6], start[7]]));
    if length > caps.header {
        return Err(FrameError::HeaderTooLarge(length));
    }
    let header = exactly(from, length)?;
    let announced = announced(&header)?;
    if announced.len() > caps.payloads {
        return Err(FrameError::TooManyPayloads(announced.len()));
    }
    if let Some(&large) = announced.iter().find(|&&n| n > caps.payload) {
        return Err(FrameError::PayloadTooLarge(large));
    }
    let payloads = announced
        .iter()
        .map(|&n| exactly(from, n))
        .collect::<Result<_, _>>()?;
    Ok(Frame { header, payloads })
}

/// The payloads `header` announces; a header that is not one JSON object
/// is refused.
fn announced(header: &[u8]) -> Result<Vec<u64>, FrameError> {
    // serde reads a struct from a JSON array as well; FH1's header is an
    // object.
    let first = header.iter().find(|b| !b.is_ascii_whitespace());
    if first != Some(&b'{') {
        return Err(FrameError::Header("not a JSON object".to_owned()));
    }
    serde_json::from_slice::<Announce>(header)
        .map(|a| a.payloads)
        .map_err(|e| FrameError::Header(e.to_string()))
}

/// Fills `buf` as far as the channel goes: the number of bytes read.
fn fill(from: &mut impl Read, buf: &mut [u8]) -> Result<usize, FrameError> {
    let mut got = 0;
    while got < buf.len() {
        match from.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e.kind().to_string())),
        }
    }
    Ok(got)
}

/// Exactly `n` bytes, the buffer growing only as they arrive.
fn exactly(from: &mut impl Read, n: u64) -> Result<Vec<u8>, FrameError> {
    let mut out = Vec::new();
    from.take(n)
        .read_to_end(&mut out)
        .map_err(|e| FrameError::Io(e.kind().to_string()))?;
    if out.len() as u64 == n {
        Ok(out)
    } else {
        Err(FrameError::Truncated)
    }
}

/// The bytes of one frame: `header` and its `payloads`, which the header
/// must announce in order.
///
/// # Errors
///
/// A header of more than 4 GiB, which no cap allows.
pub fn encode(header: &[u8], payloads: &[&[u8]]) -> io::Result<Vec<u8>> {
    let length = u32::try_from(header.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a header over 4 GiB"))?;
    let total = 8 + header.len() + payloads.iter().map(|p| p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(header);
    for payload in payloads {
        out.extend_from_slice(payload);
    }
    Ok(out)
}

/// Writes one frame to `to` and flushes it.
///
/// # Errors
///
/// The channel's, or [`encode`]'s.
pub fn write_frame(to: &mut impl Write, header: &[u8], payloads: &[&[u8]]) -> io::Result<()> {
    to.write_all(&encode(header, payloads)?)?;
    to.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(header: &str, payloads: &[&[u8]]) -> Vec<u8> {
        encode(header.as_bytes(), payloads).unwrap()
    }

    #[test]
    fn a_frame_round_trips_with_its_payloads() {
        let bytes = frame(
            r#"{"op":"scene","payloads":[3,0,2]}"#,
            &[b"abc", b"", b"de"],
        );
        let read = read_frame(&mut bytes.as_slice(), Caps::REQUEST).unwrap();
        assert_eq!(read.header, br#"{"op":"scene","payloads":[3,0,2]}"#);
        assert_eq!(read.payloads, [b"abc".to_vec(), Vec::new(), b"de".to_vec()]);
        // Two frames back to back, then the end.
        let mut two = bytes.clone();
        two.extend_from_slice(&bytes);
        let mut from = two.as_slice();
        assert!(read_frame(&mut from, Caps::REQUEST).is_ok());
        assert!(read_frame(&mut from, Caps::REQUEST).is_ok());
        assert_eq!(
            read_frame(&mut from, Caps::REQUEST),
            Err(FrameError::Closed)
        );
    }

    #[test]
    fn frames_that_break_fh1_are_refused_before_allocation() {
        let cases: Vec<(Vec<u8>, Caps, FrameError)> = vec![
            (b"FFI".to_vec(), Caps::REPLY, FrameError::Truncated),
            (b"JSON{}\0\0".to_vec(), Caps::REPLY, FrameError::Magic),
            (
                [&MAGIC[..], &(16u32 << 20 | 1).to_be_bytes()].concat(),
                Caps::REPLY,
                FrameError::HeaderTooLarge((16 << 20) + 1),
            ),
            (
                [&MAGIC[..], &u32::MAX.to_be_bytes()].concat(),
                Caps::REQUEST,
                FrameError::HeaderTooLarge(u64::from(u32::MAX)),
            ),
            (
                [&MAGIC[..], &10u32.to_be_bytes(), b"{}"].concat(),
                Caps::REPLY,
                FrameError::Truncated,
            ),
            (
                frame("[]", &[]),
                Caps::REPLY,
                FrameError::Header("not a JSON object".to_owned()),
            ),
            (
                frame(r#"{"payloads":[1]}"#, &[b"x"]),
                Caps::REPLY,
                FrameError::TooManyPayloads(1),
            ),
            (
                frame(r#"{"payloads":[1,1,1,1,1]}"#, &[]),
                Caps::REQUEST,
                FrameError::TooManyPayloads(5),
            ),
            (
                frame(r#"{"payloads":[1073741825]}"#, &[]),
                Caps::REQUEST,
                FrameError::PayloadTooLarge((1 << 30) + 1),
            ),
            // Announced and never sent: what arrived, and no more, is held.
            (
                frame(r#"{"payloads":[1073741824]}"#, &[b"abc"]),
                Caps::REQUEST,
                FrameError::Truncated,
            ),
        ];
        for (bytes, caps, expected) in cases {
            assert_eq!(
                read_frame(&mut bytes.as_slice(), caps),
                Err(expected),
                "{bytes:?}"
            );
        }
        assert!(matches!(
            read_frame(&mut frame("{", &[]).as_slice(), Caps::REPLY),
            Err(FrameError::Header(_))
        ));
        assert!(matches!(
            read_frame(
                &mut frame(r#"{"payloads":[-1]}"#, &[]).as_slice(),
                Caps::REQUEST
            ),
            Err(FrameError::Header(_))
        ));
    }

    #[test]
    fn only_the_channel_s_end_is_an_end() {
        assert!(FrameError::Closed.ended() && FrameError::Truncated.ended());
        assert!(!FrameError::Magic.ended() && !FrameError::Io("x".to_owned()).ended());
    }
}
