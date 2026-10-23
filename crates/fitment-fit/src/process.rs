//! One helper process (FL2, FL4, FL5, FH2, FY2): spawned from nothing but
//! what fitment-fit names (no inherited environment, its own working
//! directory, its three pipes and no other descriptor), with three threads
//! that only block on its pipes: a reader delivering frames over a
//! channel, a writer, and a drain keeping the last 64 KiB of its stderr for
//! the log. The waiting thread waits on the channels with the request's
//! deadline, in slices of 100 ms when a cancel flag is to be read, and
//! kills the child itself when a write or a reply misses it: a helper that
//! stops reading can never block a run.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::exit::{self, ExitClass};
use crate::frame::{self, Caps, Frame, FrameError};

/// How much of a helper's stderr is kept, for the log (FH2, FV9).
pub(crate) const STDERR_KEPT: usize = 64 << 10;

/// How often a wait reads the cancel flag (FX4).
const CANCEL_EVERY: Duration = Duration::from_millis(100);

/// How long a helper that closed its channel has to exit before it is
/// killed.
const GRACE: Duration = Duration::from_secs(2);

/// What to spawn (FL2): the program, its arguments, its working
/// directory, and whether it gets a process group of its own.
#[derive(Debug, Clone)]
pub(crate) struct Spawn {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub dir: PathBuf,
    pub own_group: bool,
}

/// How long a step may take, and the flag that cancels it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Wait<'a> {
    pub deadline: Instant,
    pub cancel: Option<&'a AtomicBool>,
}

impl<'a> Wait<'a> {
    pub fn new(within: Duration, cancel: Option<&'a AtomicBool>) -> Self {
        Self {
            deadline: Instant::now() + within,
            cancel,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

/// How a step ended without its frame (FV8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ended {
    /// A reply missed its deadline: fitment killed the helper.
    Deadline,
    /// A write missed its deadline: fitment killed the helper.
    WriteDeadline,
    /// The flag was set: fitment killed the helper.
    Cancelled,
    /// The channel carried something that is not a frame: fitment killed
    /// the helper.
    Protocol(String),
    /// The helper ended by itself, as `class`, described by `status`.
    Exited { class: ExitClass, status: String },
}

/// The last bytes of a helper's stderr.
#[derive(Debug, Default)]
struct Ring(VecDeque<u8>);

impl Ring {
    fn push(&mut self, bytes: &[u8]) {
        self.0.extend(bytes);
        let over = self.0.len().saturating_sub(STDERR_KEPT);
        self.0.drain(..over);
    }
}

/// A running helper.
#[derive(Debug)]
pub(crate) struct Process {
    child: Child,
    pid: u32,
    writer: Option<Sender<Vec<u8>>>,
    written: Receiver<std::io::Result<()>>,
    frames: Receiver<Result<Frame, FrameError>>,
    stderr: Arc<Mutex<Ring>>,
    status: Option<ExitStatus>,
    dir: PathBuf,
}

impl Process {
    /// FL2, FY2: spawns `spawn` with an environment built from nothing
    /// (only `SYSTEMROOT` on Windows), its own working directory and its
    /// three pipes; Rust's `Command` opens every other descriptor
    /// close-on-exec.
    pub fn spawn(spawn: Spawn) -> std::io::Result<Self> {
        let mut command = Command::new(&spawn.program);
        command
            .args(&spawn.args)
            .env_clear()
            .current_dir(&spawn.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SYSTEMROOT") {
            command.env("SYSTEMROOT", root);
        }
        #[cfg(unix)]
        if spawn.own_group {
            // FL5: in the server, a terminal's signals never reach it.
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(not(unix))]
        let _ = spawn.own_group;
        let mut child = command.spawn()?;
        let pid = child.id();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("a pipe was not opened"));
        };
        let (writer, written) = write_thread(pid, stdin)?;
        let frames = read_thread(pid, stdout)?;
        let ring = Arc::new(Mutex::new(Ring::default()));
        drain_thread(pid, stderr, Arc::clone(&ring))?;
        Ok(Self {
            child,
            pid,
            writer: Some(writer),
            written,
            frames,
            stderr: ring,
            status: None,
            dir: spawn.dir,
        })
    }

    /// The child's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// How it ended, once reaped.
    pub fn status(&self) -> Option<String> {
        self.status.map(exit::describe)
    }

    /// The last 64 KiB of its stderr, escaped, for the log (FV9).
    pub fn stderr(&self) -> String {
        let ring = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes: Vec<u8> = ring.0.iter().copied().collect();
        bytes.escape_ascii().to_string()
    }

    /// FL4: writes `frame` under `wait`'s deadline, on the writer thread.
    pub fn send(&mut self, frame: Vec<u8>, wait: &Wait<'_>) -> Result<(), Ended> {
        let sent = self.writer.as_ref().is_some_and(|w| w.send(frame).is_ok());
        if !sent {
            return Err(self.ended_by_itself());
        }
        match self.wait_on(Which::Written, wait) {
            Ok(Got::Written(Ok(()))) => Ok(()),
            Ok(_) => Err(self.ended_by_itself()),
            Err(Ended::Deadline) => Err(Ended::WriteDeadline),
            Err(e) => Err(e),
        }
    }

    /// FL4: the next frame under `wait`'s deadline.
    pub fn receive(&mut self, wait: &Wait<'_>) -> Result<Frame, Ended> {
        match self.wait_on(Which::Frame, wait)? {
            Got::Frame(Ok(frame)) => Ok(frame),
            Got::Frame(Err(e)) if e.ended() || matches!(e, FrameError::Io(_)) => {
                Err(self.ended_by_itself())
            }
            Got::Frame(Err(e)) => {
                self.kill();
                Err(Ended::Protocol(e.to_string()))
            }
            Got::Written(_) | Got::Gone => Err(self.ended_by_itself()),
        }
    }

    /// Kills the child, if it runs, and reaps it.
    pub fn kill(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            self.status = self.child.wait().ok();
        }
    }

    /// FL5, first half: sends `bye` and closes the channel; the helper
    /// exits on either.
    pub fn begin_finish(&mut self, bye: Option<Vec<u8>>) {
        if let (Some(writer), Some(bye)) = (&self.writer, bye) {
            let _ = writer.send(bye);
        }
        self.writer = None;
    }

    /// FL5, second half: waits for the helper to exit until `deadline`,
    /// then kills it; reaped either way.
    pub fn end_finish(&mut self, deadline: Instant) {
        while self.status.is_none() && Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => self.status = Some(status),
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        self.kill();
        self.remove_dir();
    }

    /// The helper closed its channel or a pipe failed: how it ended, once
    /// it has (FV8); one that stays up is killed, a protocol failure.
    fn ended_by_itself(&mut self) -> Ended {
        let until = Instant::now() + GRACE;
        while self.status.is_none() && Instant::now() < until {
            match self.child.try_wait() {
                Ok(Some(status)) => self.status = Some(status),
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        if let Some(status) = self.status {
            Ended::Exited {
                class: exit::classify(status),
                status: exit::describe(status),
            }
        } else {
            self.kill();
            Ended::Protocol("the helper closed its channel and stayed up".to_owned())
        }
    }

    fn wait_on(&mut self, which: Which, wait: &Wait<'_>) -> Result<Got, Ended> {
        loop {
            if wait.cancelled() {
                self.kill();
                return Err(Ended::Cancelled);
            }
            let left = wait.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.kill();
                return Err(Ended::Deadline);
            }
            let slice = if wait.cancel.is_some() {
                left.min(CANCEL_EVERY)
            } else {
                left
            };
            let got = match which {
                Which::Frame => self.frames.recv_timeout(slice).map(Got::Frame),
                Which::Written => self.written.recv_timeout(slice).map(Got::Written),
            };
            match got {
                Ok(got) => return Ok(got),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(Got::Gone),
            }
        }
    }

    fn remove_dir(&mut self) {
        if !self.dir.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.dir);
            self.dir = PathBuf::new();
        }
    }
}

impl Drop for Process {
    /// FL5: a helper is never left running, whatever ends its owner.
    fn drop(&mut self) {
        self.writer = None;
        self.kill();
        self.remove_dir();
    }
}

#[derive(Clone, Copy)]
enum Which {
    Frame,
    Written,
}

enum Got {
    Frame(Result<Frame, FrameError>),
    Written(std::io::Result<()>),
    Gone,
}

fn named(role: &str, pid: u32) -> thread::Builder {
    thread::Builder::new().name(format!("fitment-fit-{role}-{pid}"))
}

/// The writer thread: each frame it is handed, written whole; it ends,
/// closing the helper's stdin, when the sender is dropped or a write
/// fails.
type Writer = (Sender<Vec<u8>>, Receiver<std::io::Result<()>>);

fn write_thread(pid: u32, mut stdin: ChildStdin) -> std::io::Result<Writer> {
    let (to, frames) = mpsc::channel::<Vec<u8>>();
    let (done, written) = mpsc::channel();
    named("writer", pid).spawn(move || {
        for frame in frames {
            let result = stdin.write_all(&frame).and_then(|()| stdin.flush());
            let failed = result.is_err();
            if done.send(result).is_err() || failed {
                break;
            }
        }
    })?;
    Ok((to, written))
}

/// The reader thread: every frame the helper sends, until the first that
/// is not one or the channel's end.
fn read_thread(
    pid: u32,
    mut stdout: impl Read + Send + 'static,
) -> std::io::Result<Receiver<Result<Frame, FrameError>>> {
    let (to, frames) = mpsc::channel();
    named("reader", pid).spawn(move || {
        loop {
            let read = frame::read_frame(&mut stdout, Caps::REPLY);
            let last = read.is_err();
            if to.send(read).is_err() || last {
                break;
            }
        }
    })?;
    Ok(frames)
}

/// The stderr drain: everything, the last 64 KiB kept.
fn drain_thread(
    pid: u32,
    mut stderr: impl Read + Send + 'static,
    ring: Arc<Mutex<Ring>>,
) -> std::io::Result<()> {
    named("stderr", pid).spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => ring
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_last_64_kib() {
        let mut ring = Ring::default();
        ring.push(&vec![b'a'; STDERR_KEPT]);
        ring.push(b"the end");
        assert_eq!(ring.0.len(), STDERR_KEPT);
        let tail: Vec<u8> = ring.0.iter().rev().take(7).rev().copied().collect();
        assert_eq!(tail, b"the end");
    }
}
