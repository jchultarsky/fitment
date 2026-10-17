//! The accept loop (docs/research/m6-api.md, HX1, SD1, SD6): hyper's plain
//! `http1::Builder` (no `h2`, no upgrades), a header-read timeout and a
//! 64 KiB buffer, a cap on open connections, every connection watched for
//! a graceful stop; and the lease renewed while it runs.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower::ServiceExt as _;

use crate::{Server, Shared};

/// The request line and headers' buffer (HX1).
pub const MAX_BUF_SIZE: usize = 64 * 1024;

/// How long in-flight requests get once the server stops (SD1).
pub const DRAIN_REQUESTS: Duration = Duration::from_secs(10);

/// How often the lease is renewed (SD6).
pub const LEASE_RENEWAL: Duration = Duration::from_secs(10);

/// HX1's loop, until `shutdown` ends; then SD1's first and last steps.
pub(crate) async fn serve(
    server: Server,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send,
) -> std::io::Result<()> {
    let shared = Arc::clone(server.shared());
    let router = server.router();
    let graceful = GracefulShutdown::new();
    let slots = Arc::new(Semaphore::new(
        usize::try_from(shared.config.server.max_connections).unwrap_or(usize::MAX),
    ));
    let header_timeout =
        Duration::from_secs(u64::from(shared.config.server.header_timeout_seconds));
    let lease = tokio::spawn(renew(Arc::clone(&shared)));
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            () = &mut shutdown => break,
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
        let router = router.clone();
        let service = hyper::service::service_fn(move |mut request: hyper::Request<Incoming>| {
            request.extensions_mut().insert(ConnectInfo(peer));
            router.clone().oneshot(request.map(Body::new))
        });
        let connection = hyper::server::conn::http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(header_timeout)
            .max_buf_size(MAX_BUF_SIZE)
            // max_buf_size alone bounds the head only roughly (the read
            // buffer's growth let a 100 KiB head through): this is exact.
            .max_header_size(MAX_BUF_SIZE)
            .serve_connection(TokioIo::new(stream), service);
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(error = %error, "connection ended");
            }
            drop(permit);
        });
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
    let released = Arc::clone(&shared);
    let _ = tokio::task::spawn_blocking(move || released.release()).await;
    tracing::info!("stopped");
    Ok(())
}

/// SD6: renews the lease every 10 s; a failed renewal, or another holder,
/// turns readiness off (OP7) for good.
async fn renew(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(LEASE_RENEWAL);
    tick.tick().await;
    loop {
        tick.tick().await;
        let renewing = Arc::clone(&shared);
        let renewed = tokio::task::spawn_blocking(move || {
            renewing
                .db
                .renew_lease(&renewing.holder, renewing.ids.now_ms())
        })
        .await;
        let held = matches!(renewed, Ok(Ok(true)));
        if !held {
            tracing::error!("the lease could not be renewed: no longer ready");
            shared.lease_ok.store(false, Ordering::SeqCst);
            return;
        }
    }
}
