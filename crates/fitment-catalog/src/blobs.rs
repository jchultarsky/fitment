//! The blob store (`docs/research/m5-catalog.md`, ST1 to ST7, ST11): the
//! exact bytes of each content, under its SHA-256, read-only. Reachable
//! only from [`crate::Catalog`], so no reader of the index can open one
//! (QR11).

use std::io;
use std::path::PathBuf;

use crate::fs::{Content, Fs};
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
            if let Err(error) = cleared {
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
}
