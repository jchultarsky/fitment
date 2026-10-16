//! One module per command.

pub mod features;
pub mod match_;
pub mod parts;
pub mod review;
pub mod socket;
pub mod text;

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Writes `bytes` to a temporary file beside `path`, synced, then renames
/// it over `path` and syncs the directory, so a crash leaves the old file
/// or the new one, never an empty one (M5's CL7). The temporary file is
/// removed if any step fails.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let written = std::fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return written;
    }
    sync_directory(path)
}

/// Syncs the directory holding `path`, so a rename into it is durable.
/// Windows cannot open a directory to sync it; there the rename stands as
/// the file system keeps it.
fn sync_directory(path: &Path) -> io::Result<()> {
    if cfg!(windows) {
        return Ok(());
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::write_atomically;

    /// A write replaces the file whole; a write that cannot land leaves no
    /// temporary file behind.
    #[test]
    fn a_write_lands_whole_or_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!("fitment-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.json");
        write_atomically(&path, b"first, and longer").unwrap();
        write_atomically(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let missing = dir.join("absent").join("out.json");
        assert!(write_atomically(&missing, b"x").is_err());
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["out.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
