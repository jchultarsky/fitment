//! Two channels per job (docs/research/m6-api.md, JB8, JS10, M6-51): live
//! events through a `tokio::sync::broadcast` whose `send` never blocks, so
//! progress can never slow or stall a job, and persisted events also as
//! `job_event` rows. A stream subscribes first, then replays the rows after
//! `after`, then forwards live events, persisted ones deduplicated by
//! `seq`, so nothing persisted is lost between replay and live; a lagging
//! reader gets one `lagged` line and goes on; an idle stream a heartbeat;
//! a draining server a `shutdown` line (SD1). No event carries a verdict:
//! counts and states only.

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use tokio::sync::{broadcast, mpsc, watch};

use super::documents::{self, Progress};
use crate::state::EventRow;

/// A live event: a persisted one's `seq`, its type and its line.
#[derive(Debug, Clone)]
pub(crate) struct Live {
    /// Persisted events only.
    pub seq: Option<i64>,
    /// `state`, `end`, `progress`, …
    pub kind: &'static str,
    /// The NDJSON line, with its newline.
    pub line: Arc<str>,
}

/// The broadcast's capacity (JS10).
pub(crate) const CAPACITY: usize = 1024;

/// At most ten progress events a second per job (JB8).
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// Live events and progress, per job.
#[derive(Debug, Default)]
pub(crate) struct Channels {
    senders: Mutex<HashMap<i64, broadcast::Sender<Live>>>,
    progress: Mutex<HashMap<i64, (Progress, Instant)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn kind_of(name: &str) -> &'static str {
    match name {
        "state" => "state",
        "end" => "end",
        "note" => "note",
        "batch" => "batch",
        "change" => "change",
        _ => "event",
    }
}

impl Channels {
    /// A receiver of job `key`'s live events.
    pub fn subscribe(&self, key: i64) -> broadcast::Receiver<Live> {
        lock(&self.senders)
            .entry(key)
            .or_insert_with(|| broadcast::channel(CAPACITY).0)
            .subscribe()
    }

    /// Drops job `key`'s channel when no one listens and the job is not
    /// live (a stream or a wait on a job that had ended).
    pub fn release(&self, key: i64, live: bool) {
        let mut senders = lock(&self.senders);
        if !live && senders.get(&key).is_some_and(|s| s.receiver_count() == 0) {
            senders.remove(&key);
        }
    }

    /// Sends persisted events (JB8) to whoever listens; after the `end`
    /// event the channel is dropped, which closes every stream of it.
    pub fn publish(&self, job: &str, events: &[EventRow]) {
        let mut senders = lock(&self.senders);
        for event in events {
            if let Some(sender) = senders.get(&event.job) {
                let _ = sender.send(Live {
                    seq: Some(event.seq),
                    kind: kind_of(&event.kind),
                    line: documents::event_line(job, event).into(),
                });
            }
            if event.kind == "end" {
                senders.remove(&event.job);
                lock(&self.progress).remove(&event.job);
            }
        }
    }

    /// Records job `key`'s progress (`fitment.job`'s `progress`) and sends
    /// it, coalesced to at most ten a second (JB8): never a verdict.
    pub fn progress(&self, key: i64, job: &str, progress: Progress) {
        let now = Instant::now();
        let send = {
            let mut all = lock(&self.progress);
            let last = all.get(&key).map(|(_, at)| *at);
            let send = last.is_none_or(|at| now.duration_since(at) >= PROGRESS_EVERY)
                || progress.total.is_some_and(|t| progress.done >= t);
            all.insert(
                key,
                (progress, if send { now } else { last.unwrap_or(now) }),
            );
            send
        };
        if !send {
            return;
        }
        let data = serde_json::json!({
            "phase": progress.phase,
            "done": progress.done,
            "total": progress.total,
        })
        .to_string();
        if let Some(sender) = lock(&self.senders).get(&key) {
            let _ = sender.send(Live {
                seq: None,
                kind: "progress",
                line: documents::transient_line(job, "progress", &data).into(),
            });
        }
    }

    /// Job `key`'s last progress, while it runs.
    pub fn last_progress(&self, key: i64) -> Option<Progress> {
        lock(&self.progress).get(&key).map(|(p, _)| *p)
    }

    /// Forgets job `key`'s progress.
    pub fn forget(&self, key: i64) {
        lock(&self.progress).remove(&key);
    }

    /// Channels open, for the tests' leak checks.
    #[cfg(feature = "test-support")]
    pub fn open(&self) -> usize {
        lock(&self.senders).len()
    }
}

/// JS10's caps: streams per user, over all their tokens, and in all.
#[derive(Debug)]
pub(crate) struct Streams {
    per_user: usize,
    all: usize,
    open: Mutex<(usize, BTreeMap<i64, usize>)>,
}

/// A stream's slot, given back when it drops.
#[derive(Debug)]
pub(crate) struct StreamSlot {
    streams: Arc<Streams>,
    user: i64,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut open = lock(&self.streams.open);
        open.0 = open.0.saturating_sub(1);
        if let Some(n) = open.1.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.1.remove(&self.user);
            }
        }
    }
}

impl Streams {
    /// The caps.
    pub fn new(per_user: u32, all: u32) -> Self {
        Self {
            per_user: usize::try_from(per_user).unwrap_or(usize::MAX),
            all: usize::try_from(all).unwrap_or(usize::MAX),
            open: Mutex::new((0, BTreeMap::new())),
        }
    }

    /// A slot for a stream of `user`, or `None` past a cap.
    pub fn take(self: &Arc<Self>, user: i64) -> Option<StreamSlot> {
        let mut open = lock(&self.open);
        if open.0 >= self.all {
            return None;
        }
        let mine = open.1.entry(user).or_insert(0);
        if *mine >= self.per_user {
            return None;
        }
        *mine += 1;
        open.0 += 1;
        Some(StreamSlot {
            streams: Arc::clone(self),
            user,
        })
    }
}

/// A response body fed line by line from a channel: it ends when the
/// sender drops; a client gone drops it, which the sender sees as a
/// failed send and stops (VT13).
#[derive(Debug)]
pub(crate) struct ChannelBody {
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
}

impl ChannelBody {
    /// A body and its sender.
    pub fn new(capacity: usize) -> (Self, mpsc::Sender<Result<Bytes, std::io::Error>>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { rx }, tx)
    }
}

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, std::io::Error>>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(hyper::body::Frame::data(bytes)))),
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// What a stream forwards, after its replay (JS10).
pub(crate) struct Forward {
    /// The job's id.
    pub job: String,
    /// The last persisted `seq` sent.
    pub last: i64,
    /// Live events.
    pub live: broadcast::Receiver<Live>,
    /// The server's drain (SD1).
    pub shutdown: watch::Receiver<bool>,
    /// The heartbeat's interval (15 s; a seam shortens it).
    pub heartbeat: Duration,
    /// Where the lines go.
    pub out: mpsc::Sender<Result<Bytes, std::io::Error>>,
}

impl Forward {
    async fn send(&self, line: &str) -> bool {
        self.out
            .send(Ok(Bytes::copy_from_slice(line.as_bytes())))
            .await
            .is_ok()
    }

    /// Forwards live events until the `end` event, the drain, or the
    /// client's leaving (JS10): persisted ones past `last` only; `lagged`
    /// when the reader fell behind; a heartbeat when idle.
    pub async fn run(mut self) {
        loop {
            let next = tokio::time::timeout(self.heartbeat, self.live.recv());
            let received = tokio::select! {
                received = next => received,
                _ = self.shutdown.changed() => {
                    let _ = self.send(&documents::transient_line(&self.job, "shutdown", "{}")).await;
                    return;
                }
                () = self.out.closed() => return,
            };
            match received {
                Err(_) => {
                    if !self
                        .send(&documents::transient_line(&self.job, "heartbeat", "{}"))
                        .await
                    {
                        return;
                    }
                }
                Ok(Ok(event)) => {
                    if event.seq.is_some_and(|s| s <= self.last) {
                        continue;
                    }
                    if !self.send(&event.line).await {
                        return;
                    }
                    if let Some(seq) = event.seq {
                        self.last = seq;
                    }
                    if event.kind == "end" {
                        return;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(missed))) => {
                    let data = format!(r#"{{"missed":{missed}}}"#);
                    if !self
                        .send(&documents::transient_line(&self.job, "lagged", &data))
                        .await
                    {
                        return;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(seq: i64, kind: &'static str) -> Live {
        Live {
            seq: Some(seq),
            kind,
            line: format!("{{\"seq\":{seq}}}\n").into(),
        }
    }

    /// JS10: a reader that fell behind gets one `lagged` line, with how
    /// many it missed, and goes on from the next event to the end; an event
    /// it already replayed is not sent twice.
    #[tokio::test]
    async fn a_lagging_reader_is_told_and_goes_on() {
        let (sender, receiver) = broadcast::channel(2);
        for seq in 1..=5 {
            sender.send(live(seq, "state")).unwrap();
        }
        sender.send(live(6, "end")).unwrap();
        let (out, mut rx) = mpsc::channel(16);
        let (_drain, shutdown) = watch::channel(false);
        let forward = Forward {
            job: "j_x".into(),
            last: 5,
            live: receiver,
            shutdown,
            heartbeat: Duration::from_secs(60),
            out,
        };
        forward.run().await;
        let mut lines = Vec::new();
        while let Ok(Ok(bytes)) = rx.try_recv() {
            lines.push(String::from_utf8(bytes.to_vec()).unwrap());
        }
        assert!(lines[0].contains("\"type\":\"lagged\""), "{lines:?}");
        assert!(lines[0].contains("\"missed\":4"), "{lines:?}");
        assert!(!lines[0].contains("\"seq\""), "transient: no seq");
        assert_eq!(
            lines[1..],
            ["{\"seq\":6}\n".to_owned()],
            "5 was replayed already"
        );
    }
}
