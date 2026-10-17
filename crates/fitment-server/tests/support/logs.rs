//! A tracing capture for in-process tests (XH13): every event of the test
//! binary, the blocking pool's included, written as JSON lines into one
//! buffer, which tests scan for secrets.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

static CAPTURE: OnceLock<Buffer> = OnceLock::new();

/// Installs the capture as the global subscriber, once per test binary.
pub fn capture() {
    CAPTURE.get_or_init(|| {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_current_span(true)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        buffer
    });
}

/// Every line captured so far.
pub fn text() -> String {
    CAPTURE
        .get()
        .map(|b| String::from_utf8_lossy(&b.0.lock().unwrap()).into_owned())
        .unwrap_or_default()
}

static PLANTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Registers a string a test plants in the log on purpose (VT34's
/// controls): the end-of-test scan does not count its lines.
pub fn plant(text: &str) {
    PLANTED.lock().unwrap().push(text.to_owned());
}

/// XH13's log half at the end of every server a test made: the secrets in
/// the whole captured log, but for the lines a control planted.
pub fn leaks() -> Vec<String> {
    let planted = PLANTED.lock().unwrap().clone();
    text()
        .lines()
        .filter(|line| !planted.iter().any(|p| line.contains(p.as_str())))
        .flat_map(super::scan::logs)
        .collect()
}
