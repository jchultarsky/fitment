//! The blob store (`docs/research/m5-catalog.md`, ST1 to ST7, ST11): the
//! exact bytes of each content, under its SHA-256, read-only. Reachable
//! only from [`crate::Catalog`], so no reader of the index can open one
//! (QR11).

use std::io;
use std::path::PathBuf;

use crate::fs::{Content, Fs, FsRead};
use crate::hex::Sha256Hex;
use crate::layout::Layout;

/// How a blob came to be in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    /// Written under its name.
    Written,
    /// Already there with the same bytes: reused.
    Reused,
    /// There with other bytes, which the new write replaced (ST3).
    Replaced,
}

/// Why a blob could not be read whole and checked (ST11).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BadBlob {
    /// No file at its name.
    #[error("missing: no file for the content")]
    Missing,
    /// A file whose SHA-256 is not its name, or not a regular file.
    #[error("damaged: the file's SHA-256 is not its name")]
    Damaged,
    /// The file could not be read.
    #[error("cannot read the file: {0}")]
    Io(#[from] io::Error),
}

impl BadBlob {
    /// What a line says of it: an I/O error by its kind alone (CA4).
    pub(crate) fn kind_text(&self) -> String {
        match self {
            Self::Io(e) => format!("cannot read the file: {}", crate::io_kind(e)),
            other => other.to_string(),
        }
    }
}

/// The blobs of one catalog.
pub(crate) struct Blobs<'a> {
    pub(crate) layout: &'a Layout,
    pub(crate) fs: &'a dyn Fs,
}

impl Blobs<'_> {
    /// ST3: puts `bytes`, whose SHA-256 is `sha256`, at its final name: a
    /// temporary file created new, written and synced, renamed into place
    /// and made read-only; or the file already there, if it is a regular
    /// file that hashes to its name, made read-only (ST4). The fan-out
    /// directory is synced by the caller, once per commit (ST3, review F6).
    /// Every error path removes the temporary file this call created, and
    /// no other (ST7).
    pub(crate) fn put(&self, sha256: &Sha256Hex, bytes: &[u8]) -> io::Result<Placed> {
        // A gc may unlink an orphan of these bytes between any two steps
        // (it checks only that no row names it): NotFound starts again,
        // and ST6's check under the lock settles what a commit names.
        for _ in 0..2 {
            match self.put_once(sha256, bytes) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                done => return done,
            }
        }
        self.put_once(sha256, bytes)
    }

    fn put_once(&self, sha256: &Sha256Hex, bytes: &[u8]) -> io::Result<Placed> {
        let path = self.layout.blob(sha256);
        let there = match self.read(sha256) {
            Ok(_) => {
                self.fs.set_readonly(&path, true)?;
                return Ok(Placed::Reused);
            }
            Err(BadBlob::Missing) => false,
            Err(_) => true,
        };
        let temporary = self.temporary(bytes)?;
        if there {
            // Never follow what stands there: a link or a directory is
            // unlinked; a regular file loses its read-only flag, which
            // Windows needs before a rename over it.
            let regular = self.fs.symlink_metadata(&path).is_ok_and(|m| m.is_file());
            let cleared = if regular {
                self.fs.set_readonly(&path, false)
            } else {
                self.fs.remove_file(&path)
            };
            let gone = |e: &io::Error| e.kind() == io::ErrorKind::NotFound;
            if let Err(error) = cleared.or_else(|e| if gone(&e) { Ok(()) } else { Err(e) }) {
                let _ = self.fs.remove_file(&temporary);
                return Err(error);
            }
        }
        if let Err(error) = self.fs.rename(&temporary, &path) {
            let _ = self.fs.remove_file(&temporary);
            // CC2: another writer may have put the same bytes there first.
            if self.read(sha256).is_ok() {
                return Ok(Placed::Reused);
            }
            return Err(error);
        }
        self.fs.set_readonly(&path, true)?;
        Ok(if there {
            Placed::Replaced
        } else {
            Placed::Written
        })
    }

    /// A temporary file holding `bytes`, written and synced, under a name
    /// no other file has: a name already taken (a crashed writer's, or
    /// another process's with the same id) is passed over, never removed.
    fn temporary(&self, bytes: &[u8]) -> io::Result<PathBuf> {
        let mut taken = 0;
        loop {
            let temporary = self.layout.temporary();
            match self.fs.write_new(&temporary, bytes) {
                Ok(()) => return Ok(temporary),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && taken < 100 => taken += 1,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
                Err(e) => {
                    let _ = self.fs.remove_file(&temporary);
                    return Err(e);
                }
            }
        }
    }

    /// ST6: whether the blob is there with `bytes` bytes.
    pub(crate) fn present(&self, sha256: &Sha256Hex, bytes: u64) -> bool {
        self.fs
            .symlink_metadata(&self.layout.blob(sha256))
            .is_ok_and(|m| m.is_file() && m.len() == bytes)
    }

    /// Syncs the fan-out directory of `sha256` (ST3).
    pub(crate) fn sync_fan_out(&self, sha256: &Sha256Hex) -> io::Result<()> {
        self.fs.sync_dir(&self.layout.fan_out(sha256))
    }

    /// ST11: the blob read once into one buffer, and that buffer only if it
    /// hashes to its name.
    pub(crate) fn read(&self, sha256: &Sha256Hex) -> Result<Vec<u8>, BadBlob> {
        let path = self.layout.blob(sha256);
        match self.fs.symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(BadBlob::Missing),
            Err(e) => return Err(BadBlob::Io(e)),
            Ok(meta) if !meta.is_file() => return Err(BadBlob::Damaged),
            Ok(_) => {}
        }
        match self.fs.read_capped(&path, u64::MAX)? {
            Content::Bytes(bytes) if Sha256Hex::of(&bytes) == *sha256 => Ok(bytes),
            _ => Err(BadBlob::Damaged),
        }
    }

    /// ST11 for a stream (`docs/research/m6-api.md`, OB3): the blob hashed
    /// in one pass of a reader, then rewound on the same open file for the
    /// caller's pass, which is hashed again as it goes and fails its last
    /// read, `InvalidData`, if the file changed meanwhile; nothing is held
    /// but a chunk. Answers its length and the reader.
    pub(crate) fn open_checked(
        &self,
        sha256: &Sha256Hex,
    ) -> Result<(u64, Box<dyn FsRead>), BadBlob> {
        let path = self.layout.blob(sha256);
        let length = match self.fs.symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(BadBlob::Missing),
            Err(e) => return Err(BadBlob::Io(e)),
            Ok(meta) if !meta.is_file() => return Err(BadBlob::Damaged),
            Ok(meta) => meta.len(),
        };
        let opened = self.fs.open_stream(&path, length).map_err(stream_error);
        let mut reader = opened?;
        let mut hasher = fitment_core::Sha256::new();
        let mut buf = vec![0_u8; 256 * 1024];
        let mut seen = 0_u64;
        loop {
            let n = reader.read_chunk(&mut buf).map_err(stream_error)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            seen += u64::try_from(n).unwrap_or(u64::MAX);
        }
        if seen != length || hasher.finish_hex() != sha256.as_str() {
            return Err(BadBlob::Damaged);
        }
        reader.rewind()?;
        let checked = Checked {
            inner: reader,
            hasher: fitment_core::Sha256::new(),
            sha256: sha256.clone(),
            length,
            seen: 0,
        };
        Ok((length, Box::new(checked)))
    }
}

/// A blob's file gone is [`BadBlob::Missing`]; one not regular, or longer
/// than it was, damaged.
fn stream_error(error: io::Error) -> BadBlob {
    match error.kind() {
        io::ErrorKind::NotFound => BadBlob::Missing,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => BadBlob::Damaged,
        _ => BadBlob::Io(error),
    }
}

/// OB3's second pass over a blob: hashed again as it is read; the read
/// that meets the end fails if the bytes were not the blob's.
struct Checked {
    inner: Box<dyn FsRead>,
    hasher: fitment_core::Sha256,
    sha256: Sha256Hex,
    length: u64,
    seen: u64,
}

impl FsRead for Checked {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read_chunk(buf)?;
        if n == 0 {
            let hash = std::mem::take(&mut self.hasher).finish_hex();
            if self.seen != self.length || hash != self.sha256.as_str() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the blob changed between its check and its reading",
                ));
            }
            return Ok(0);
        }
        self.hasher.update(&buf[..n]);
        self.seen += u64::try_from(n).unwrap_or(u64::MAX);
        Ok(n)
    }

    fn rewind(&mut self) -> io::Result<()> {
        self.inner.rewind()?;
        self.hasher = fitment_core::Sha256::new();
        self.seen = 0;
        Ok(())
    }
}
