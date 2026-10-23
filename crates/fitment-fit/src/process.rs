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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
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

/// A deadline too far for `Instant` is this far instead.
const FAR: Duration = Duration::from_secs(365 * 24 * 3600);

/// How long a report waits for the stderr drain to read what a reaped
/// helper left in its pipe (the review of PR 2a): bounded, since a child
/// of the helper may hold the pipe open.
const DRAIN: Duration = Duration::from_millis(500);

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
    /// `within` from now; a span past what `Instant` holds waits a year,
    /// never panics (the review of PR 2a, P1).
    pub fn new(within: Duration, cancel: Option<&'a AtomicBool>) -> Self {
        let now = Instant::now();
        let deadline = now
            .checked_add(within)
            .or_else(|| now.checked_add(FAR))
            .unwrap_or(now);
        Self { deadline, cancel }
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
    drain: Option<thread::JoinHandle<()>>,
    status: Option<ExitStatus>,
    dir: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    own_group: bool,
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
        if spawn.own_group {
            // FL5: in the server, a terminal's signals never reach it.
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            #[cfg(windows)]
            {
                // CREATE_NEW_PROCESS_GROUP: a console's Ctrl-C passes it by.
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0000_0200);
            }
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        // The review of PR 2a (L1): a thread that cannot start leaves no
        // child behind.
        let threads = (|| {
            let (Some(stdin), Some(stdout), Some(stderr)) =
                (child.stdin.take(), child.stdout.take(), child.stderr.take())
            else {
                return Err(std::io::Error::other("a pipe was not opened"));
            };
            let (writer, written) = write_thread(pid, stdin)?;
            let frames = read_thread(pid, stdout)?;
            let ring = Arc::new(Mutex::new(Ring::default()));
            let drain = drain_thread(pid, stderr, Arc::clone(&ring))?;
            Ok((writer, written, frames, ring, drain))
        })();
        let (writer, written, frames, ring, drain) = match threads {
            Ok(threads) => threads,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        Ok(Self {
            child,
            pid,
            writer: Some(writer),
            written,
            frames,
            stderr: ring,
            drain: Some(drain),
            status: None,
            dir: spawn.dir,
            own_group: spawn.own_group,
        })
    }

    /// The review of PR 2a (P5): a frame the helper sent unasked, or its
    /// end, while it sat idle; `None` if it sent nothing.
    pub fn unasked(&self) -> Option<String> {
        match self.frames.try_recv() {
            Ok(Ok(_)) => Some("the helper sent a frame no request asked for".to_owned()),
            Ok(Err(e)) => Some(format!("the helper's channel broke while idle: {e}")),
            Err(TryRecvError::Disconnected) => {
                Some("the helper's channel closed while idle".to_owned())
            }
            Err(TryRecvError::Empty) => None,
        }
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

    /// Kills the child, if it runs, and its process group if it has one of
    /// its own (Unix), and reaps it; then gives the stderr drain a moment
    /// to read what the helper left (bounded).
    pub fn kill(&mut self) {
        if self.status.is_none() {
            #[cfg(unix)]
            if self.own_group {
                // The review of PR 2a (L2): std kills one process; the
                // group, the helper's children with it, by kill(1) on its
                // negative pgid, which is the helper's pid.
                let _ = Command::new(KILL)
                    .args(["-s", "KILL", "--", &format!("-{}", self.pid)])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = self.child.kill();
            self.status = self.child.wait().ok();
        }
        self.settle_stderr();
    }

    /// Waits, at most [`DRAIN`], for the stderr drain to finish.
    fn settle_stderr(&mut self) {
        let until = Instant::now() + DRAIN;
        while self.drain.as_ref().is_some_and(|d| !d.is_finished()) && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        if self
            .drain
            .as_ref()
            .is_some_and(thread::JoinHandle::is_finished)
        {
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
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
            remove_dir(&self.dir);
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

/// A spawn handed to the spawner thread, and where its process goes.
type Spawning = (Spawn, Sender<std::io::Result<Process>>);

/// FL5: every helper of a checked helper is spawned from one thread that
/// lives as long as the checked helper does. On Linux the helper asks for
/// `PR_SET_PDEATHSIG`, which follows the *thread* that spawned it, not the
/// process: spawned from a judging thread, it would be killed when that
/// thread ended, while another thread still had it from the pool.
#[derive(Debug, Default)]
pub(crate) struct Spawner {
    to: Mutex<Option<Sender<Spawning>>>,
}

impl Spawner {
    /// `spawn`, from the spawner's thread; from this one if no thread can
    /// be started.
    pub fn spawn(&self, spawn: Spawn) -> std::io::Result<Process> {
        let (back, answer) = mpsc::channel();
        let sent = {
            let mut to = self.to.lock().unwrap_or_else(PoisonError::into_inner);
            if to.is_none() {
                *to = Self::thread();
            }
            to.as_ref()
                .is_some_and(|to| to.send((spawn.clone(), back)).is_ok())
        };
        match sent.then(|| answer.recv().ok()).flatten() {
            Some(spawned) => spawned,
            None => Process::spawn(spawn),
        }
    }

    /// The thread, which ends when the spawner is dropped.
    fn thread() -> Option<Sender<Spawning>> {
        let (to, from) = mpsc::channel::<Spawning>();
        thread::Builder::new()
            .name("fitment-fit-spawner".to_owned())
            .spawn(move || {
                for (spawn, back) in from {
                    let _ = back.send(Process::spawn(spawn));
                }
            })
            .ok()?;
        Some(to)
    }
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

/// The kill(1) a process group is killed with (Unix).
#[cfg(unix)]
const KILL: &str = "/bin/kill";

/// Removes a helper's directory; on Windows, retried for a moment, since
/// a scanner may still hold a file the helper wrote (the review of PR 2a,
/// L5).
pub(crate) fn remove_dir(dir: &std::path::Path) {
    let tries = if cfg!(windows) { 20 } else { 1 };
    for _ in 0..tries {
        match std::fs::remove_dir_all(dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                thread::sleep(Duration::from_millis(50));
            }
            _ => return,
        }
    }
}

/// The reader thread: every frame the helper sends, until the first that
/// is not one or the channel's end. Its channel holds one frame, so a
/// helper that floods it waits in its own pipe rather than in fitment's
/// memory (the review of PR 2a, P5).
fn read_thread(
    pid: u32,
    mut stdout: impl Read + Send + 'static,
) -> std::io::Result<Receiver<Result<Frame, FrameError>>> {
    let (to, frames) = mpsc::sync_channel(1);
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
) -> std::io::Result<thread::JoinHandle<()>> {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reviews of PR 2a (P1): a deadline past what `Instant` holds
    /// waits far, and never panics.
    #[test]
    fn a_deadline_too_far_never_panics() {
        let far = Wait::new(Duration::MAX, None);
        assert!(far.deadline > Instant::now() + Duration::from_secs(3600));
    }

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
