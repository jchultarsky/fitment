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
