//! CA7 (docs/research/m6-api.md, VT38): the streaming writer and reader of
//! the `Fs` seam. Every chunk, sync and read is a call `CountingFs`
//! records and `failing_at` fails, so the server's object store, built on
//! them, can be failed at each of its file calls in turn (VT11).

mod common;

use std::io;
use std::path::{Path, PathBuf};

use fitment_catalog::fs::{Call, CountingFs};
use fitment_catalog::{Fs, StdFs};

/// A fresh directory, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        Self(common::scratch(&format!("fs-{name}")))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes `chunks` to `path` through `fs`, synced; reads it back in
/// pieces of 7 bytes.
fn round_trip(fs: &dyn Fs, path: &Path, chunks: &[&[u8]]) -> io::Result<Vec<u8>> {
    let mut writer = fs.create_new_stream(path)?;
    for chunk in chunks {
        writer.write_chunk(chunk)?;
    }
    writer.sync()?;
    drop(writer);
    let mut reader = fs.open_stream(path, 1 << 20)?;
    let mut out = Vec::new();
    let mut buf = [0_u8; 7];
    loop {
        let n = reader.read_chunk(&mut buf)?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// CA7: each streaming call is recorded by `CountingFs` in order (the
/// create, every chunk with its length, the sync, the open and every
/// read), the bytes read are the bytes written, and `failing_at` fails
/// each call in turn, with nothing read past the failure.
#[test]
fn streams_go_through_the_seam() {
    let dir = Dir::new("seam");
    let path = dir.0.join("a.part");
    let seam = CountingFs::new();
    let chunks: [&[u8]; 3] = [b"hello, ", b"streamed ", b"world"];
    assert_eq!(
        round_trip(&seam, &path, &chunks).unwrap(),
        b"hello, streamed world"
    );
    let calls = seam.calls();
    assert_eq!(
        calls[..5],
        [
            Call::CreateStream(path.clone()),
            Call::WriteChunk(path.clone(), 7),
            Call::WriteChunk(path.clone(), 9),
            Call::WriteChunk(path.clone(), 5),
            Call::SyncStream(path.clone()),
        ]
    );
    assert_eq!(calls[5], Call::OpenStream(path.clone()));
    // 21 bytes in pieces of 7, then the end.
    assert_eq!(
        calls[6..],
        [
            Call::ReadChunk(path.clone(), 7),
            Call::ReadChunk(path.clone(), 7),
            Call::ReadChunk(path.clone(), 7),
            Call::ReadChunk(path.clone(), 7),
        ]
    );
    for k in 0..calls.len() {
        for kind in [io::ErrorKind::StorageFull, io::ErrorKind::Other] {
            let file = dir.0.join(format!("f{k}.part"));
            let failing = CountingFs::failing_at(k, kind);
            let error = round_trip(&failing, &file, &chunks).unwrap_err();
            assert_eq!(error.kind(), kind, "call {k}");
            assert_eq!(failing.calls().len(), k + 1, "nothing after call {k}");
            let _ = std::fs::remove_file(&file);
        }
    }
    // A clone shares the count, streams included.
    let shared = CountingFs::new();
    let other = shared.clone();
    round_trip(&other, &dir.0.join("b.part"), &chunks).unwrap();
    assert_eq!(shared.calls().len(), calls.len());
}

/// CA7: a stream is created new, never over a file, with mode 0600 on
/// Unix; an open refuses what is not a regular file and what is longer
/// than its cap, and a file that grows past its cap after the open fails
/// the read that meets the extra byte.
#[test]
fn streams_are_new_private_regular_and_capped() {
    let dir = Dir::new("rules");
    let path = dir.0.join("x.part");
    let mut writer = StdFs.create_new_stream(&path).unwrap();
    writer.write_chunk(b"0123456789").unwrap();
    writer.sync().unwrap();
    drop(writer);
    let again = StdFs.create_new_stream(&path).err().map(|e| e.kind());
    assert_eq!(again, Some(io::ErrorKind::AlreadyExists));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let kind = |r: io::Result<Box<dyn fitment_catalog::FsRead>>| r.err().map(|e| e.kind());
    assert_eq!(
        kind(StdFs.open_stream(&dir.0, 100)),
        Some(io::ErrorKind::InvalidInput)
    );
    assert_eq!(
        kind(StdFs.open_stream(&path, 9)),
        Some(io::ErrorKind::InvalidData)
    );
    let mut reader = StdFs.open_stream(&path, 10).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut f| io::Write::write_all(&mut f, b"!"))
        .unwrap();
    let mut buf = [0_u8; 64];
    assert_eq!(reader.read_chunk(&mut buf).unwrap(), 10);
    assert_eq!(&buf[..10], b"0123456789");
    let grown = reader.read_chunk(&mut buf).err().map(|e| e.kind());
    assert_eq!(grown, Some(io::ErrorKind::InvalidData));
}

/// CA7 (PR 4's review): a stream rewinds on its open file, so a second
/// pass reads the bytes the first did, and an empty directory standing at
/// a name is removed; both are calls the seam records and fails.
#[test]
fn a_stream_rewinds_and_a_directory_is_removed_through_the_seam() {
    let dir = Dir::new("rewind");
    let path = dir.0.join("r.part");
    let seam = CountingFs::new();
    let mut writer = seam.create_new_stream(&path).unwrap();
    writer.write_chunk(b"0123456789").unwrap();
    writer.sync().unwrap();
    drop(writer);
    let mut reader = seam.open_stream(&path, 10).unwrap();
    let mut buf = [0_u8; 16];
    assert_eq!(reader.read_chunk(&mut buf).unwrap(), 10);
    reader.rewind().unwrap();
    assert_eq!(reader.read_chunk(&mut buf).unwrap(), 10, "the cap again");
    assert_eq!(&buf[..10], b"0123456789");
    let standing = dir.0.join("standing");
    std::fs::create_dir(&standing).unwrap();
    seam.remove_dir(&standing).unwrap();
    assert!(!standing.exists());
    let calls = seam.calls();
    assert!(calls.contains(&Call::Rewind(path.clone())));
    assert!(calls.contains(&Call::RemoveDir(standing.clone())));
    let k = calls
        .iter()
        .position(|c| *c == Call::Rewind(path.clone()))
        .unwrap();
    let failing = CountingFs::failing_at(k, io::ErrorKind::Other);
    let other = dir.0.join("f.part");
    let mut writer = failing.create_new_stream(&other).unwrap();
    writer.write_chunk(b"0123456789").unwrap();
    writer.sync().unwrap();
    drop(writer);
    let mut reader = failing.open_stream(&other, 10).unwrap();
    reader.read_chunk(&mut buf).unwrap();
    assert!(reader.rewind().is_err());
}
