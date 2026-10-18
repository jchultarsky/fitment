//! The accept loop (docs/research/m6-api.md, HX1, SD1, SD6): hyper's plain
//! `http1::Builder` (no `h2`, no upgrades), a header-read timeout and a
//! 64 KiB head, a cap on open connections, a write that must make
//! progress and a longest life for each connection (so a client that never
//! reads cannot hold a slot), every connection watched for a graceful
//! stop; and the lease renewed while it runs, its loss stopping the
//! server.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tower::ServiceExt as _;

use crate::{Server, Shared};

/// The request line and headers' buffer (HX1).
pub const MAX_BUF_SIZE: usize = 64 * 1024;

/// How long in-flight requests get once the server stops (SD1).
pub const DRAIN_REQUESTS: Duration = Duration::from_secs(10);

/// How often the lease is renewed (SD6).
pub const LEASE_RENEWAL: Duration = Duration::from_secs(10);

/// How long renewals may fail for a transient reason (a busy database)
/// before the lease counts as lost: the time after which another server
/// may take it (SD6).
pub const LEASE_GRACE: Duration = Duration::from_secs(30);

/// HX1's loop, until `shutdown` ends or the lease is lost; then SD1's
/// first and last steps. Losing the lease (another holder, or no renewal
/// for [`LEASE_GRACE`]) stops the server with an error, so the process
/// exits non-zero (SD6).
pub(crate) async fn serve(
    server: Server,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    let shared = Arc::clone(server.shared());
    let router = server.router();
    let graceful = GracefulShutdown::new();
    let slots = Arc::new(Semaphore::new(
        usize::try_from(shared.config.server.max_connections).unwrap_or(usize::MAX),
    ));
    let (lost_tx, mut lost) = tokio::sync::oneshot::channel::<()>();
    let lease = tokio::spawn(renew(Arc::clone(&shared), lost_tx));
    let sweep = tokio::spawn(sweep_auth(Arc::clone(&shared)));
    let mut lease_lost = false;
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            () = &mut shutdown => break,
            _ = &mut lost => {
                lease_lost = true;
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // EMFILE and the like: wait a moment rather than spin.
                    tracing::warn!(error = %error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
        };
        // A connection past the cap is closed at accept (HX1).
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            tracing::warn!("connection refused: server.max_connections reached");
            drop(stream);
            continue;
        };
        connection(&shared, &router, &graceful, stream, peer, permit);
    }
    server.drain();
    drop(listener);
    if tokio::time::timeout(DRAIN_REQUESTS, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("in-flight requests outlived the drain");
    }
    lease.abort();
    sweep.abort();
    let released = Arc::clone(&shared);
    let _ = tokio::task::spawn_blocking(move || released.release()).await;
    if lease_lost {
        tracing::error!("stopped: the state directory's lease was lost");
        return Err(io::Error::other("the state directory's lease was lost"));
    }
    tracing::info!("stopped");
    Ok(())
}

/// Serves one accepted connection on its own task: hyper's limits (HX1),
/// a write that must progress within `server.write_timeout_seconds`, and
/// at most `server.max_connection_seconds` in all; its permit is returned
/// when it ends.
fn connection(
    shared: &Arc<Shared>,
    router: &axum::Router,
    graceful: &GracefulShutdown,
    stream: TcpStream,
    peer: std::net::SocketAddr,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let server = &shared.config.server;
    let header_timeout = Duration::from_secs(u64::from(server.header_timeout_seconds));
    let write_timeout = Duration::from_secs(u64::from(server.write_timeout_seconds));
    let lifetime = Duration::from_secs(u64::from(server.max_connection_seconds));
    let router = router.clone();
    let service = hyper::service::service_fn(move |mut request: hyper::Request<Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        router.clone().oneshot(request.map(Body::new))
    });
    let io = WriteDeadline {
        io: stream,
        timeout: write_timeout,
        stalled: None,
    };
    let connection = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(header_timeout)
        .max_buf_size(MAX_BUF_SIZE)
        // max_buf_size alone bounds the head only roughly (the read
        // buffer's growth let a 100 KiB head through): this is exact.
        .max_header_size(MAX_BUF_SIZE)
        .serve_connection(TokioIo::new(io), service);
    let connection = graceful.watch(connection);
    tokio::spawn(async move {
        match tokio::time::timeout(lifetime, connection).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::debug!(error = %error, "connection ended"),
            Err(_) => tracing::info!("connection closed: server.max_connection_seconds reached"),
        }
        drop(permit);
    });
}

/// A stream whose writes must make progress (review S1): a write, flush or
/// shutdown pending for longer than `timeout` without one byte accepted
/// fails with `TimedOut`, and hyper closes the connection, so a client
/// that stops reading its response cannot hold a connection slot.
struct WriteDeadline<T> {
    io: T,
    timeout: Duration,
    stalled: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<T> WriteDeadline<T> {
    fn watch<R>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<io::Result<R>>,
    ) -> Poll<io::Result<R>> {
        if polled.is_ready() {
            self.stalled = None;
            return polled;
        }
        let timeout = self.timeout;
        let sleep = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
        if sleep.as_mut().poll(cx).is_ready() {
            self.stalled = None;
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "a write made no progress",
            )));
        }
        Poll::Pending
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for WriteDeadline<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for WriteDeadline<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.io).poll_write(cx, buf);
        this.watch(cx, polled)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.io).poll_write_vectored(cx, bufs);
        this.watch(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.io).poll_flush(cx);
        this.watch(cx, polled)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.io).poll_shutdown(cx);
        this.watch(cx, polled)
    }
}

/// SD6: renews the lease every [`LEASE_RENEWAL`] (a `test-support` seam
/// shortens it), in a `BEGIN IMMEDIATE`. A transient failure (a busy
/// database) is retried at the next tick; the lease is lost when another
/// holder has it, or when no renewal succeeded for [`LEASE_GRACE`]. Then
/// readiness turns off (OP7) and `lost` stops the accept loop.
async fn renew(shared: Arc<Shared>, lost: tokio::sync::oneshot::Sender<()>) {
    let mut tick = tokio::time::interval(shared.lease_renewal());
    tick.tick().await;
    let mut renewed_at = tokio::time::Instant::now();
    loop {
        tick.tick().await;
        let renewing = Arc::clone(&shared);
        let renewed = tokio::task::spawn_blocking(move || {
            renewing
                .db
                .renew_lease(&renewing.holder, renewing.ids.now_ms())
        })
        .await;
        match renewed {
            Ok(Ok(true)) => renewed_at = tokio::time::Instant::now(),
            Ok(Ok(false)) => {
                tracing::error!("another server took the state directory's lease");
                break;
            }
            Ok(Err(error)) if renewed_at.elapsed() < LEASE_GRACE => {
                tracing::warn!(error = %error, "the lease could not be renewed; retrying");
            }
            Err(_) if renewed_at.elapsed() < LEASE_GRACE => {
                tracing::warn!("the lease's renewal failed; retrying");
            }
            _ => {
                tracing::error!("the lease could not be renewed for 30 s");
                break;
            }
        }
    }
    shared.lease_ok.store(false, Ordering::SeqCst);
    let _ = lost.send(());
}

/// The auth part of JS9's sweep, every `jobs.sweep_minutes` (AU6, audit
/// flaw 4): the batched `last_used_ms` written, tokens dead for 30 days
/// deleted. PR 5's sweep takes it over; a graceful stop writes the batch
/// a last time (SD1).
async fn sweep_auth(shared: Arc<Shared>) {
    let every = Duration::from_secs(shared.config.jobs.sweep_minutes.max(1).saturating_mul(60));
    let mut tick = tokio::time::interval(every);
    tick.tick().await;
    loop {
        tick.tick().await;
        let sweeping = Arc::clone(&shared);
        match tokio::task::spawn_blocking(move || sweeping.sweep_auth()).await {
            Ok(Ok(deleted)) => tracing::info!(deleted, "swept the tokens"),
            Ok(Err(error)) => tracing::warn!(error = %error, "the tokens could not be swept"),
            Err(_) => tracing::warn!("the tokens' sweep failed"),
        }
    }
}
