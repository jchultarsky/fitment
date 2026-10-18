//! Where the reports go (`docs/research/m4-match.md`, KH6;
//! `docs/research/m6-api.md`, XH6): `target/fitment-corpus/`, one file
//! per path, so CI can diff the library's against the API's.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// `target/fitment-corpus` in the repository.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fitment-corpus")
}

/// Writes `report` (`Score::report`) as `name` in [`dir`]: pretty JSON,
/// as `report.json` has always been written.
pub fn write(name: &str, report: &Value) -> io::Result<PathBuf> {
    fs::create_dir_all(dir())?;
    let path = dir().join(name);
    fs::write(&path, serde_json::to_string_pretty(report)?)?;
    Ok(path)
}
