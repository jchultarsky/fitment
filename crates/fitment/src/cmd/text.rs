//! Text for people: escaping what a file states, and numbers.

use std::io::{self, Write};

/// `text` with every control character escaped, so that text from a file
/// cannot start a new line or move the terminal: one reason, one line. The
/// Unicode line and paragraph separators and the bidirectional controls
/// are escaped too: they split or reorder lines in some terminals and log
/// viewers.
pub fn printable(text: &str) -> String {
    let unsafe_format = |c: char| {
        matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
    };
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() || unsafe_format(c) {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// Writes one line to stderr. A closed stderr is not worth a panic.
pub fn note(line: &str) {
    let _ = writeln!(io::stderr().lock(), "{line}");
}

/// A number for people: up to 4 decimals, trailing zeros dropped.
pub fn mm(x: f64) -> String {
    let text = format!("{x:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

pub fn vector(v: [f64; 3]) -> String {
    format!("({}, {}, {})", mm(v[0]), mm(v[1]), mm(v[2]))
}
