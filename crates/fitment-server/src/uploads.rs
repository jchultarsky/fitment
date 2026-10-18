//! Uploads (docs/research/m6-api.md, section 4.5, UP1 to UP4, UP8, HX3):
//! bytes enter the server only here. A body is consumed as a stream, each
//! chunk counted, fed to CO1's hasher and written to a temporary file
//! through CA7's streaming writer, all on the blocking pool (SV3); the cap,
//! the quotas, the next chunk's wait, the whole upload's time and the
//! minimum throughput are checked as it streams, and any refusal or abort
//! removes the temporary file. At the end the file is synced, then stored
//! as UP8 says ([`crate::state::Db::store_upload`]).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, header};
use http_body_util::BodyExt as _;
use serde::Serialize;
use tokio::time::Instant;

use crate::Shared;
use crate::config::Limits;
use crate::http::blocking;
use crate::http::error::{ApiError, ErrorCode};
use crate::state::{Holding, Staged, Synced};

/// `fitment.upload`.
pub const UPLOAD_FORMAT: &str = "fitment.upload";

/// Its version (DC9: may grow within v1).
pub const UPLOAD_FORMAT_VERSION: u32 = 1;

/// The media types an upload's body may declare (section 7.3).
pub const UPLOAD_TYPES: [&str; 3] = ["application/octet-stream", "model/step", "application/json"];

/// Chunks are gathered to this many bytes before one blocking write.
const FLUSH: usize = 1024 * 1024;

/// One MiB.
const MIB: u64 = 1024 * 1024;

/// `fitment.upload` v1 (section 7.11): what of the caller's holds an
/// object, never another user's holdings (UP3).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UploadDocument {
    /// `fitment.upload`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// The bytes' SHA-256, lower-case hex.
    pub sha256: String,
    /// Their length.
    pub bytes: u64,
    /// The caller's rows that hold them: `upload`, `assembly`, `socket`,
    /// `job` (OB4).
    pub held_by: Vec<&'static str>,
    /// How many of the caller's jobs hold them as an input (OB4).
    pub pinned_by_jobs: u64,
}

impl UploadDocument {
    /// The document of `holding`.
    pub fn of(holding: &Holding) -> Self {
        Self {
            format: UPLOAD_FORMAT,
            format_version: UPLOAD_FORMAT_VERSION,
            sha256: holding.sha256.clone(),
            bytes: holding.bytes,
            held_by: holding.held_by.clone(),
            pinned_by_jobs: holding.pinned_by_jobs,
        }
    }
}

/// UP4's counts: uploads at once, per server and per user over all their
/// tokens (review F5), and the bytes of open temporary files.
#[derive(Debug)]
pub struct Uploads {
    all: usize,
    per_user: usize,
    running: Mutex<(usize, BTreeMap<i64, usize>)>,
    staged: AtomicU64,
    staging_cap: u64,
}

/// Why an upload found no slot (UP4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// `limits.upload_concurrency` uploads run on the server.
    Server,
    /// `limits.upload_concurrency_per_user` run for the user.
    User,
}

impl Uploads {
    /// The counts for `limits`.
    pub fn new(limits: &Limits) -> Self {
        Self {
            all: usize::try_from(limits.upload_concurrency).unwrap_or(usize::MAX),
            per_user: usize::try_from(limits.upload_concurrency_per_user).unwrap_or(usize::MAX),
            running: Mutex::new((0, BTreeMap::new())),
            staged: AtomicU64::new(0),
            staging_cap: limits.staging_gib.saturating_mul(1024 * MIB),
        }
    }

    /// A slot for an upload of `user`, held until the slot drops.
    ///
    /// # Errors
    ///
    /// [`Busy`].
    pub fn take(self: &Arc<Self>, user: i64) -> Result<Slot, Busy> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if running.0 >= self.all {
            return Err(Busy::Server);
        }
        let mine = running.1.entry(user).or_insert(0);
        if *mine >= self.per_user {
            return Err(Busy::User);
        }
        *mine += 1;
        running.0 += 1;
        Ok(Slot {
            uploads: Arc::clone(self),
            user,
        })
    }

    /// The bytes of open temporary files.
    pub fn staged(&self) -> u64 {
        self.staged.load(Ordering::SeqCst)
    }

    /// Adds `n` to the staged bytes if they stay within the staging quota.
    fn reserve(&self, n: u64) -> bool {
        let mut staged = self.staged.load(Ordering::SeqCst);
        loop {
            let Some(total) = staged.checked_add(n).filter(|t| *t <= self.staging_cap) else {
                return false;
            };
            match self.staged.compare_exchange_weak(
                staged,
                total,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(now) => staged = now,
            }
        }
    }

    fn release(&self, n: u64) {
        let mut staged = self.staged.load(Ordering::SeqCst);
        loop {
            let left = staged.saturating_sub(n);
            match self.staged.compare_exchange_weak(
                staged,
                left,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(now) => staged = now,
            }
        }
    }

    /// The uploads running: on the server, and for `user`.
    pub fn running(&self, user: i64) -> (usize, usize) {
        let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        (running.0, running.1.get(&user).copied().unwrap_or(0))
    }
}

/// An upload's slot (UP4); dropped, the slot is free.
#[derive(Debug)]
pub struct Slot {
    uploads: Arc<Uploads>,
    user: i64,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut running = self
            .uploads
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        running.0 = running.0.saturating_sub(1);
        if let Some(mine) = running.1.get_mut(&self.user) {
            *mine = mine.saturating_sub(1);
            if *mine == 0 {
                running.1.remove(&self.user);
            }
        }
    }
}

/// The staged bytes an upload counts against the staging quota (UP4);
/// released when its temporary file is gone, so when this drops.
#[derive(Debug)]
pub struct Reservation {
    uploads: Arc<Uploads>,
    bytes: u64,
}

impl Reservation {
    /// Counts `n` more bytes; false if that would pass the quota.
    fn grow(&mut self, n: u64) -> bool {
        let ok = self.uploads.reserve(n);
        if ok {
            self.bytes += n;
        }
        ok
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.uploads.release(self.bytes);
    }
}

/// A temporary file in flight, owned by the async side: dropped there
/// (the request cut by a layer's timeout, or the client gone), it is
/// dropped on the blocking pool, which removes the file (SV3, UP2).
struct Pending(Option<Staged>);

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(staged) = self.0.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => drop(runtime.spawn_blocking(move || drop(staged))),
                Err(_) => drop(staged),
            }
        }
    }
}

/// What an upload received: its temporary file synced and named, and its
/// slot and staged bytes, kept until it is stored or discarded.
#[derive(Debug)]
pub struct Received {
    /// The temporary file.
    pub synced: Synced,
    /// Its slot (UP4).
    pub slot: Slot,
    /// Its staged bytes (UP4).
    pub reservation: Reservation,
}

/// `413 too_large`: the upload cap (UP2, SZ1).
fn too_large(cap: u64) -> ApiError {
    ApiError::new(
        ErrorCode::TooLarge,
        format!("an upload is at most {} MiB", cap / MIB),
    )
    .with("limit", "max_upload_mib")
    .with("max_bytes", cap)
}

/// `507 quota_exceeded`, for `limit` (UP4).
fn over_quota(limit: &'static str, detail: &'static str) -> ApiError {
    ApiError::new(ErrorCode::QuotaExceeded, detail).with("limit", limit)
}

/// `408 upload_too_slow` (HX3, review F5): retryable, nothing kept.
fn too_slow(detail: String) -> ApiError {
    ApiError::new(ErrorCode::UploadTooSlow, detail)
        .with("limit", "upload_min_kib_s")
        .retry_after(1)
}

/// A storage error while writing (UP4, F9): `507 storage_full` for a full
/// disk, else `500 io_error`; the cause logged, never sent (ER3).
pub fn storage(error: &std::io::Error) -> ApiError {
    if error.kind() == std::io::ErrorKind::StorageFull {
        tracing::warn!("an upload met a full disk");
        ApiError::new(
            ErrorCode::StorageFull,
            "the server's storage is full: nothing was kept",
        )
    } else {
        tracing::error!(kind = ?error.kind(), "an upload could not be written");
        ApiError::new(
            ErrorCode::IoError,
            "the upload could not be written: nothing was kept",
        )
    }
}

/// The upload cap in bytes (UP2): `limits.max_upload_mib`.
pub fn cap(limits: &Limits) -> u64 {
    limits.max_upload_mib.saturating_mul(MIB)
}

/// A declared `Content-Length`, if one is given and well formed.
pub fn declared(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// UP2: a declared length over the cap is `413 too_large` before a byte
/// is read.
///
/// # Errors
///
/// `too_large`.
pub fn check_declared(headers: &HeaderMap, limits: &Limits) -> Result<(), ApiError> {
    let cap = cap(limits);
    match declared(headers) {
        Some(n) if n > cap => Err(too_large(cap)),
        _ => Ok(()),
    }
}

/// Section 7.3, DC6: the body is one of [`UPLOAD_TYPES`] (parameters
/// allowed), and no `Content-Encoding` is taken: `415`.
fn check_type(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::CONTENT_ENCODING) {
        return Err(ApiError::new(
            ErrorCode::UnsupportedMediaType,
            "request bodies are never decompressed: send it without Content-Encoding",
        ));
    }
    let media = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    if media.is_some_and(|m| UPLOAD_TYPES.iter().any(|t| t.eq_ignore_ascii_case(m))) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::UnsupportedMediaType,
            "an upload's body is application/octet-stream, model/step or application/json",
        )
        .with("accepted", UPLOAD_TYPES.to_vec()))
    }
}

/// HX3's minimum throughput: the bytes received in the last window, from
/// the samples of (time, bytes so far), must be at least `need` once the
/// upload is a window old.
#[derive(Debug)]
struct Throughput {
    started: Instant,
    window: Duration,
    need: u64,
    samples: VecDeque<(Instant, u64)>,
}

impl Throughput {
    fn new(started: Instant, limits: &Limits) -> Self {
        let window = Duration::from_secs(limits.upload_window_seconds);
        let need = limits
            .upload_min_kib_s
            .saturating_mul(1024)
            .saturating_mul(limits.upload_window_seconds);
        Self {
            started,
            window,
            need,
            samples: VecDeque::from([(started, 0)]),
        }
    }

    /// Records `total` bytes received at `now`.
    fn record(&mut self, now: Instant, total: u64) {
        self.samples.push_back((now, total));
    }

    /// True if, at `now` with `total` bytes, the last window fell short.
    fn short(&mut self, now: Instant, total: u64) -> bool {
        let Some(since) = now.checked_sub(self.window) else {
            return false;
        };
        if since < self.started {
            return false;
        }
        // The base is the last sample at or before the window's start;
        // older ones are no longer needed.
        while self.samples.len() > 1 && self.samples[1].0 <= since {
            self.samples.pop_front();
        }
        let base = self.samples.front().map_or(0, |s| s.1);
        total.saturating_sub(base) < self.need
    }

    /// How often the throughput is checked while no chunk comes.
    fn every(&self) -> Duration {
        (self.window / 30).clamp(Duration::from_millis(100), Duration::from_secs(1))
    }
}

/// The checks before a byte is read (UP4): the body's type, a slot, the
/// user's quota and the staging quota for a declared length.
async fn admit(
    shared: &Arc<Shared>,
    user: i64,
    headers: &HeaderMap,
) -> Result<(Slot, Reservation, u64), ApiError> {
    check_type(headers)?;
    let limits = &shared.config.limits;
    check_declared(headers, limits)?;
    // SD1: a draining server takes no new upload.
    if shared.draining.load(Ordering::SeqCst) {
        return Err(ApiError::new(
            ErrorCode::ShuttingDown,
            "the server is draining: send the upload to another server or later",
        )
        .with("reason", "draining")
        .retry_after(5));
    }
    let slot = shared.uploads.take(user).map_err(|busy| {
        let (limit, detail) = match busy {
            Busy::Server => (
                "upload_concurrency",
                "the server takes no more uploads at once: try again later",
            ),
            Busy::User => (
                "upload_concurrency_per_user",
                "this user's uploads at once are at their limit: try again later",
            ),
        };
        ApiError::new(ErrorCode::TooManyStreams, detail)
            .with("limit", limit)
            .retry_after(1)
    })?;
    let reading = Arc::clone(shared);
    let held = blocking(move || {
        reading
            .db
            .held_bytes(user, reading.ids.now_ms())
            .map_err(|e| crate::http::db_error(&e))
    })
    .await?;
    let declared = declared(headers).unwrap_or(0);
    let quota = limits.user_quota_gib.saturating_mul(1024 * MIB);
    if held.saturating_add(declared) > quota {
        return Err(over_quota(
            "user_quota_gib",
            "this user's stored bytes are at their quota: delete some first",
        ));
    }
    let mut reservation = Reservation {
        uploads: Arc::clone(&shared.uploads),
        bytes: 0,
    };
    if !reservation.grow(declared) {
        return Err(over_quota(
            "staging_gib",
            "the server's staging space is full: try again later",
        ));
    }
    Ok((slot, reservation, held))
}

/// UP2, UP4, HX3: receives `body` for `user` into a temporary file,
/// synced and named by its SHA-256.
///
/// # Errors
///
/// `415`, `429 too_many_streams`, `507 quota_exceeded` before a byte is
/// read; `413 too_large`, `507 quota_exceeded`, `408 upload_too_slow`,
/// `503 request_timeout`, `507 storage_full` or `500 io_error` while it
/// streams. The temporary file is removed in every case.
pub(crate) async fn receive(
    shared: &Arc<Shared>,
    user: i64,
    headers: &HeaderMap,
    body: Body,
) -> Result<Received, ApiError> {
    let (slot, mut reservation, held) = admit(shared, user, headers).await?;
    let staging = Arc::clone(shared);
    let staged = blocking(move || staging.objects.stage().map_err(|e| storage(&e))).await?;
    let mut pending = Pending(Some(staged));
    let streamed = stream(shared, &mut pending, &mut reservation, held, headers, body).await;
    let finished = match streamed {
        Ok(()) => match pending.0.take() {
            Some(staged) => blocking(move || staged.finish().map_err(|e| storage(&e))).await,
            None => Err(ApiError::internal()),
        },
        Err(error) => Err(error),
    };
    match finished {
        Ok(synced) => Ok(Received {
            synced,
            slot,
            reservation,
        }),
        Err(error) => {
            // The temporary file removed before the answer (UP2).
            if let Some(staged) = pending.0.take() {
                let _ = blocking(move || {
                    drop(staged);
                    Ok(())
                })
                .await;
            }
            Err(error)
        }
    }
}

/// The streaming half of [`receive`]: the body read frame by frame under
/// HX3's waits and UP2's and UP4's counts, written in pieces of at most
/// [`FLUSH`] bytes.
async fn stream(
    shared: &Arc<Shared>,
    pending: &mut Pending,
    reservation: &mut Reservation,
    held: u64,
    headers: &HeaderMap,
    mut body: Body,
) -> Result<(), ApiError> {
    let limits = &shared.config.limits;
    let (cap, quota) = (
        cap(limits),
        limits.user_quota_gib.saturating_mul(1024 * MIB),
    );
    let started = Instant::now();
    let deadline = started + Duration::from_secs(limits.upload_minutes.saturating_mul(60));
    let chunk_wait = Duration::from_secs(limits.upload_chunk_seconds);
    let mut throughput = Throughput::new(started, limits);
    let (mut last_chunk, mut received) = (started, 0_u64);
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        #[cfg(feature = "test-support")]
        shared.hooks.reached("upload.chunk").await;
        let next = (last_chunk + chunk_wait)
            .min(deadline)
            .min(Instant::now() + throughput.every());
        let Ok(frame) = tokio::time::timeout_at(next, body.frame()).await else {
            let now = Instant::now();
            if now >= deadline {
                return Err(whole_upload(limits));
            }
            if now >= last_chunk + chunk_wait || throughput.short(now, received) {
                return Err(slow(limits, received));
            }
            continue;
        };
        let Some(frame) = frame else { break };
        let frame = frame.map_err(|_| {
            ApiError::new(ErrorCode::BadRequest, "the upload's body could not be read")
        })?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        let now = Instant::now();
        last_chunk = now;
        received = received.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if received > cap {
            return Err(too_large(cap));
        }
        if held.saturating_add(received) > quota {
            return Err(over_quota(
                "user_quota_gib",
                "this user's stored bytes are at their quota: delete some first",
            ));
        }
        if received > reservation.bytes && !reservation.grow(received - reservation.bytes) {
            return Err(over_quota(
                "staging_gib",
                "the server's staging space is full: try again later",
            ));
        }
        throughput.record(now, received);
        if throughput.short(now, received) {
            return Err(slow(limits, received));
        }
        buffer.extend_from_slice(&chunk);
        if buffer.len() >= FLUSH {
            write(pending, std::mem::take(&mut buffer)).await?;
        }
    }
    if !buffer.is_empty() {
        write(pending, buffer).await?;
    }
    // hyper enforces a declared length on the wire; a body handed in
    // whole is held to it too.
    if declared(headers).is_some_and(|n| n != received) {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            "the body's length is not its Content-Length",
        ));
    }
    Ok(())
}

/// `503 request_timeout`: the whole upload past `limits.upload_minutes`
/// (HX3), retryable on the upload's `PUT` (ER6).
fn whole_upload(limits: &Limits) -> ApiError {
    ApiError::new(
        ErrorCode::RequestTimeout,
        format!("an upload takes at most {} minutes", limits.upload_minutes),
    )
    .with("limit", "upload_minutes")
}

/// `408 upload_too_slow` for an upload that received `received` bytes.
fn slow(limits: &Limits, received: u64) -> ApiError {
    too_slow(format!(
        "an upload sends at least {} KiB/s over any {} s, and a chunk within {} s; \
         {received} bytes came",
        limits.upload_min_kib_s, limits.upload_window_seconds, limits.upload_chunk_seconds
    ))
    .with("received", received)
}

/// Writes `chunk` to the pending file on the blocking pool; after a
/// failure the file stays pending, for [`receive`] to remove.
async fn write(pending: &mut Pending, chunk: Vec<u8>) -> Result<(), ApiError> {
    let mut staged = pending.0.take().ok_or_else(ApiError::internal)?;
    let (staged, written) = blocking(move || {
        let written = staged.write(&chunk);
        Ok((staged, written))
    })
    .await?;
    pending.0 = Some(staged);
    written.map_err(|e| storage(&e))
}
