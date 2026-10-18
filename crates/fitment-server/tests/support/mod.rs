//! The server tests' harness (docs/research/m6-api.md, section 9.4): a
//! state and a catalog directory per test, the server over three
//! transports (`oneshot` in process, a bound socket, the built binary as
//! a child), one client over all three, and on every response XH11's
//! document checks and XH13's leak scan, so no test can forget them.
#![allow(dead_code)] // each test binary uses part of it
#![allow(clippy::disallowed_methods)] // tests read the environment and spawn threads

pub mod logs;
pub mod scan;

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Read as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use fitment_server::{Config, Flags, Server};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

/// The prefix of every directory a test makes, so XH13's scan recognizes
/// its paths in a response.
pub const PREFIX: &str = "fitment-test-";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A test's directories: a catalog (an M5 catalog, made empty) and a
/// state directory (made by the server's first start), under one root
/// removed when dropped.
#[derive(Debug)]
pub struct Dirs {
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub state: PathBuf,
}

impl Dirs {
    /// Fresh directories, the catalog initialised.
    pub fn new() -> Self {
        let dirs = Self::bare();
        fitment_catalog::Catalog::init(&dirs.catalog, fitment_catalog::Options::default())
            .expect("an empty catalog");
        dirs
    }

    /// Fresh directories, no catalog made.
    pub fn bare() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("{PREFIX}{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Resolved, so a path the server writes is the one the scan seeks
        // (macOS's temporary directory is behind a link). Windows'
        // canonical form is a verbatim `\\?\` path, which nothing here needs.
        #[cfg(unix)]
        let root = std::fs::canonicalize(&root).unwrap();
        Self {
            catalog: root.join("catalog"),
            state: root.join("state"),
            root,
        }
    }

    /// The configuration for these directories, listening on
    /// `127.0.0.1:0`.
    pub fn config(&self) -> Config {
        let flags = Flags {
            catalog: Some(self.catalog.clone()),
            state: Some(self.state.clone()),
            listen: Some("127.0.0.1:0".into()),
            ..Flags::default()
        };
        Config::from_sources(&flags, &std::collections::BTreeMap::new()).unwrap()
    }

    /// The paths XH13's scan refuses in a response.
    pub fn roots(&self) -> Vec<String> {
        vec![
            self.root.display().to_string(),
            self.catalog.display().to_string(),
            self.state.display().to_string(),
        ]
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One response, read whole.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    /// A header's value as text.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The error document's code, checking the problem's shape (DC9, ER1).
    pub fn code(&self) -> String {
        assert_eq!(
            self.header("content-type"),
            Some("application/problem+json"),
            "{}",
            String::from_utf8_lossy(&self.body)
        );
        let doc = self.json();
        assert_eq!(doc["format"], "fitment.error");
        assert_eq!(doc["status"], self.status.as_u16());
        doc["code"].as_str().unwrap().to_owned()
    }
}

/// A request to send. Unless it names its own credentials or is
/// [`Call::anonymous`], [`TestServer::send`] presents the harness's admin
/// token (AU5: every route but four and login needs one).
#[derive(Debug, Clone)]
pub struct Call {
    pub method: Method,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Sent without the harness's token.
    pub anonymous: bool,
    /// In process, the peer's address (`ConnectInfo`), which AU7 reads.
    pub peer: Option<SocketAddr>,
}

impl Call {
    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
            anonymous: false,
            peer: None,
        }
    }

    /// A JSON request.
    pub fn json(method: Method, path: &str, body: &serde_json::Value) -> Self {
        Self::new(method, path)
            .header("content-type", "application/json")
            .body(body.to_string().as_bytes())
    }

    /// Without the harness's token.
    #[must_use]
    pub fn anonymous(mut self) -> Self {
        self.anonymous = true;
        self
    }

    /// With `Authorization: Bearer <token>`.
    #[must_use]
    pub fn bearer(self, token: &str) -> Self {
        self.header("authorization", &format!("Bearer {token}"))
    }

    /// From `peer`, in process.
    #[must_use]
    pub fn peer(mut self, peer: SocketAddr) -> Self {
        self.peer = Some(peer);
        self
    }

    fn names_credentials(&self) -> bool {
        self.anonymous
            || self
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("authorization"))
    }

    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub fn body(mut self, body: &[u8]) -> Self {
        self.body = body.to_vec();
        self
    }

    fn request(&self, authority: Option<SocketAddr>) -> Request<Body> {
        let uri = match authority {
            Some(addr) => format!("http://{addr}{}", self.path),
            None => self.path.clone(),
        };
        let mut builder = Request::builder().method(self.method.clone()).uri(uri);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if !self.body.is_empty()
            && !self
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        {
            builder = builder.header("content-length", self.body.len().to_string());
        }
        let mut request = builder.body(Body::from(self.body.clone())).unwrap();
        if let Some(peer) = self.peer {
            request
                .extensions_mut()
                .insert(axum::extract::ConnectInfo(peer));
        }
        request
    }
}

/// How the client reaches the server.
enum Transport {
    /// `Server::router().oneshot` (section 9.4's router layer).
    InProcess(axum::Router),
    /// A bound socket or a child process, over `hyper-util`'s client.
    Wire(SocketAddr),
}

/// A server under test and the client that reaches it.
pub struct TestServer {
    pub dirs: Dirs,
    pub server: Option<Server>,
    /// The harness's admin token, made on first use (AU9's library).
    token: std::sync::OnceLock<String>,
    transport: Transport,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    pub child: Option<Process>,
}

impl TestServer {
    /// In process, with fixed ids and time (OP11), so bodies are exact.
    pub fn in_process(dirs: Dirs, config: Config) -> Self {
        Self::in_process_with_fs(dirs, config, Arc::new(fitment_catalog::StdFs))
    }

    /// [`TestServer::in_process`] with the object store on `fs`, a
    /// `CountingFs` that records or fails its file calls (OP11, VT11).
    pub fn in_process_with_fs(
        dirs: Dirs,
        config: Config,
        fs: Arc<dyn fitment_catalog::Fs>,
    ) -> Self {
        logs::capture();
        let server = Server::build_with_fs(config, fs).expect("the server builds");
        server.seams().ids.fix_ids();
        server.seams().ids.fix_time(1_790_000_000_000);
        let router = server.router();
        Self {
            dirs,
            server: Some(server),
            token: std::sync::OnceLock::new(),
            transport: Transport::InProcess(router),
            stop: None,
            task: None,
            child: None,
        }
    }

    /// The default configuration, in process.
    pub fn fresh() -> Self {
        let dirs = Dirs::new();
        let config = dirs.config();
        Self::in_process(dirs, config)
    }

    /// On a bound socket, on this runtime (section 9.4's socket layer).
    pub async fn bound(dirs: Dirs, config: Config) -> Self {
        Self::bound_with(dirs, config, |_| {}).await
    }

    /// As [`TestServer::bound`], `prepare` run on the server before it
    /// serves (a seam such as the lease's renewal).
    pub async fn bound_with(dirs: Dirs, config: Config, prepare: impl FnOnce(&Server)) -> Self {
        logs::capture();
        let server = Server::build(config).expect("the server builds");
        prepare(&server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = server.clone();
        let task = tokio::spawn(serving.serve(listener, async move {
            let _ = stopped.await;
        }));
        Self {
            dirs,
            server: Some(server),
            token: std::sync::OnceLock::new(),
            transport: Transport::Wire(addr),
            stop: Some(stop),
            task: Some(task),
            child: None,
        }
    }

    /// The built binary as a child (section 9.4's process layer).
    pub fn process(dirs: Dirs, config_toml: &str) -> Self {
        let child = Process::start(&dirs, config_toml, &[]).expect("the server starts");
        let addr = child.addr;
        Self {
            dirs,
            server: None,
            token: std::sync::OnceLock::new(),
            transport: Transport::Wire(addr),
            stop: None,
            task: None,
            child: Some(child),
        }
    }

    /// The address, for a bound or process server.
    pub fn addr(&self) -> SocketAddr {
        match &self.transport {
            Transport::Wire(addr) => *addr,
            Transport::InProcess(_) => panic!("an in-process server has no address"),
        }
    }

    pub fn server(&self) -> &Server {
        self.server.as_ref().expect("an in-process or bound server")
    }

    /// The harness's admin token: a user `harness` made with AU9's
    /// offline library on this server's state directory, with its own
    /// random ids, so a golden's fixed ids are untouched.
    pub fn token(&self) -> &str {
        self.token.get_or_init(|| {
            let minted = admin(&self.dirs)
                .bootstrap_or_mint("harness")
                .expect("the harness's token");
            scan::secret(minted.secret.expose());
            minted.secret.expose().to_owned()
        })
    }

    /// Sends `call`; the response passes XH11's checks and XH13's scan.
    pub async fn send(&self, call: Call) -> Reply {
        let reply = self.send_unchecked(call).await;
        let problems = scan::response(&reply, &self.dirs.roots());
        assert!(
            problems.is_empty(),
            "the response leaks or fails its checks: {problems:?}"
        );
        reply
    }

    /// Sends `call` without the checks: for VT34's controls only.
    pub async fn send_unchecked(&self, call: Call) -> Reply {
        let call = if call.names_credentials() {
            call
        } else {
            let token = self.token().to_owned();
            call.bearer(&token)
        };
        match &self.transport {
            Transport::InProcess(router) => {
                let response = router.clone().oneshot(call.request(None)).await.unwrap();
                let (parts, body) = response.into_parts();
                let body = body.collect().await.unwrap().to_bytes().to_vec();
                Reply {
                    status: parts.status,
                    headers: parts.headers,
                    body,
                }
            }
            Transport::Wire(addr) => wire(*addr, &call).await,
        }
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.send(Call::get(path)).await
    }

    /// Stops a bound server and waits for its loop to end.
    /// Waits up to `limit` for a bound server's loop to end by itself:
    /// its result, or `None` if it still runs.
    pub async fn finished(&mut self, limit: Duration) -> Option<std::io::Result<()>> {
        let task = self.task.as_mut()?;
        let result = tokio::time::timeout(limit, task).await.ok()?;
        self.task = None;
        Some(result.unwrap())
    }

    pub async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.unwrap().unwrap();
        }
    }
}

/// One request over `hyper-util`'s client (M6-40).
pub async fn wire(addr: SocketAddr, call: &Call) -> Reply {
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http::<Body>();
    let response = client.request(call.request(Some(addr))).await.unwrap();
    let (parts, body) = response.into_parts();
    let body = body.collect().await.unwrap().to_bytes().to_vec();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

impl Drop for TestServer {
    /// XH13's log half (review S9): the whole log the test binary captured,
    /// and a child's stderr, hold no secret, but for planted lines.
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let mut found = logs::leaks();
        if let Some(child) = &self.child {
            found.extend(scan::logs(&child.stderr.lock().unwrap()));
        }
        assert!(found.is_empty(), "a secret in the log: {found:?}");
    }
}

/// The built `fitment-server` binary (`CARGO_BIN_EXE_fitment-server`).
pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_fitment-server")
}

/// A running `fitment-server serve`: its address read from stdout's one
/// line, its stderr collected, stopped by closing its stdin (OP11).
pub struct Process {
    pub child: Child,
    pub addr: SocketAddr,
    stdin: Option<ChildStdin>,
    pub stdout: Arc<Mutex<String>>,
    pub stderr: Arc<Mutex<String>>,
}

/// What a start that failed printed.
#[derive(Debug)]
pub struct Failed {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn collect(stream: impl std::io::Read + Send + 'static) -> Arc<Mutex<String>> {
    let out = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&out);
    std::thread::spawn(move || {
        let mut text = String::new();
        let mut reader = BufReader::new(stream);
        let mut buf = [0_u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    text.push_str(&String::from_utf8_lossy(&buf[..n]));
                    sink.lock().unwrap().clone_from(&text);
                }
            }
        }
    });
    out
}

impl Process {
    /// Writes `config_toml` (with `paths` for `dirs` if it names none) and
    /// starts `serve --config FILE` plus `args`; waits up to 60 s for the
    /// address line.
    pub fn start(dirs: &Dirs, config_toml: &str, args: &[&str]) -> Result<Self, Failed> {
        let file = write_config(dirs, config_toml);
        let mut command = Command::new(binary());
        command
            .args(["serve", "--config"])
            .arg(&file)
            .args(args)
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, _) in std::env::vars() {
            if name.starts_with("FITMENT_SERVER_") {
                command.env_remove(name);
            }
        }
        let mut child = command.spawn().expect("spawn fitment-server");
        let stdin = child.stdin.take();
        let stderr = collect(child.stderr.take().unwrap());
        let mut lines = BufReader::new(child.stdout.take().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let rest = Arc::new(Mutex::new(String::new()));
        let rest_sink = Arc::clone(&rest);
        std::thread::spawn(move || {
            let mut first = String::new();
            let _ = lines.read_line(&mut first);
            let _ = tx.send(first);
            let mut tail = String::new();
            let _ = lines.read_to_string(&mut tail);
            rest_sink.lock().unwrap().push_str(&tail);
        });
        let first = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_default();
        let addr = first
            .strip_prefix("fitment-server: listening on ")
            .and_then(|a| a.trim_end().parse().ok());
        if let Some(addr) = addr {
            rest.lock().unwrap().push_str(&first);
            Ok(Self {
                child,
                addr,
                stdin,
                stdout: rest,
                stderr,
            })
        } else {
            drop(stdin);
            let status = wait(&mut child, Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(100));
            let stderr = stderr.lock().unwrap().clone();
            Err(Failed {
                code: status,
                stdout: first,
                stderr,
            })
        }
    }

    /// Stops it gracefully (stdin's end of file, OP11) and returns its exit
    /// code, its stdout and its stderr.
    pub fn stop(mut self) -> (Option<i32>, String, String) {
        drop(self.stdin.take());
        let code = wait(&mut self.child, Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(100));
        let stdout = self.stdout.lock().unwrap().clone();
        let stderr = self.stderr.lock().unwrap().clone();
        (code, stdout, stderr)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Waits for `child` up to `limit`, killing it after.
pub fn wait(child: &mut Child, limit: Duration) -> Option<i32> {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return status.code();
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            return child.wait().ok().and_then(|s| s.code());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Writes a configuration file in `dirs.root`, adding `[paths]` for the
/// test's directories unless the text names its own.
pub fn write_config(dirs: &Dirs, config_toml: &str) -> PathBuf {
    let mut text = config_toml.to_owned();
    if !text.contains("[paths]") {
        let _ = write!(
            text,
            "\n[paths]\ncatalog = {:?}\nstate = {:?}\n",
            dirs.catalog.display().to_string(),
            dirs.state.display().to_string()
        );
    }
    if !text.contains("listen") {
        text = format!("[server]\nlisten = \"127.0.0.1:0\"\n{text}");
    }
    let file = dirs.root.join("server.toml");
    std::fs::write(&file, text).unwrap();
    file
}

/// Runs the binary with `args` to its end: its exit code, stdout and
/// stderr.
pub fn run(args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let mut command = Command::new(binary());
    command.args(args).stdin(Stdio::null());
    for (name, _) in std::env::vars() {
        if name.starts_with("FITMENT_SERVER_") {
            command.env_remove(name);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A header name for the request id.
pub fn request_id() -> HeaderName {
    HeaderName::from_static("x-request-id")
}

/// True if `path` holds `server.db` and the marker.
pub fn is_state(path: &Path) -> bool {
    path.join("server.db").is_file() && path.join("FITMENT-SERVER-STATE").is_file()
}

/// A header value.
pub fn value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).unwrap()
}

/// AU9's library on `dirs`'s state directory, without a pepper.
pub fn admin(dirs: &Dirs) -> fitment_server::admin::Admin {
    fitment_server::admin::Admin::open(&dirs.config()).expect("the state opens offline")
}

/// The harness's ways with the offline library.
pub trait AdminExt {
    /// Bootstraps `username` as the first admin, or, when an admin exists,
    /// makes it an admin and mints its token.
    fn bootstrap_or_mint(
        &self,
        username: &str,
    ) -> Result<fitment_server::auth::Minted, fitment_server::admin::AdminError>;
}

impl AdminExt for fitment_server::admin::Admin {
    fn bootstrap_or_mint(
        &self,
        username: &str,
    ) -> Result<fitment_server::auth::Minted, fitment_server::admin::AdminError> {
        match self.bootstrap(username, None) {
            Err(fitment_server::admin::AdminError::AdminExists) => {
                let _ = self.add_user(username, fitment_server::auth::Role::Admin, None);
                self.mint(username, "harness", None, None)
            }
            other => other,
        }
    }
}

/// A password for a test, registered so no response or log may hold it
/// (XH13).
pub fn password(text: &str) -> String {
    scan::secret(text);
    text.to_owned()
}

/// A person made through the server's own seams (its ids and clock):
/// `password` hashed with the server's pepper and registered as a secret.
pub fn seed_user(
    server: &Server,
    username: &str,
    role: fitment_server::auth::Role,
    password: Option<&str>,
) -> fitment_server::state::UserRow {
    let seams = server.seams();
    let phc = password.map(|p| {
        scan::secret(p);
        fitment_server::auth::password::hash(p, seams.auth.pepper()).unwrap()
    });
    let id = seams.ids.id("u");
    seams
        .db
        .create_user(
            &id,
            username,
            &username.to_ascii_lowercase(),
            role,
            phc.as_deref(),
            seams.ids.now_ms(),
        )
        .unwrap()
}

/// A token of `user` made through the seams, with no parent, of `role`,
/// living from the server's now for `days` (negative: expired); its
/// secret registered.
pub fn seed_token(
    server: &Server,
    user: &fitment_server::state::UserRow,
    role: fitment_server::auth::Role,
    days: i64,
) -> (fitment_server::state::TokenRow, String) {
    let seams = server.seams();
    let now = seams.ids.now_ms();
    let minted = fitment_server::auth::mint(
        seams.db,
        seams.ids,
        &fitment_server::state::MintFrom::Offline { user: user.key },
        "seeded",
        role,
        now,
        now + days * fitment_server::auth::DAY_MS,
        u32::MAX,
    )
    .unwrap();
    scan::secret(minted.secret.expose());
    (minted.row, minted.secret.expose().to_owned())
}

/// A JSON body's call with a token.
pub fn post(path: &str, token: &str, body: &serde_json::Value) -> Call {
    Call::json(Method::POST, path, body).bearer(token)
}

/// An upload of `bytes` by `owner`, stored through the server's own store
/// and clock as an upload route stores one (UP2, UP8), living a day:
/// returns its SHA-256.
pub fn seed_upload(server: &Server, owner: i64, bytes: &[u8]) -> String {
    let seams = server.seams();
    let mut staged = seams.objects.stage().unwrap();
    staged.write(bytes).unwrap();
    let synced = staged.finish().unwrap();
    let now = seams.ids.now_ms();
    let sha256 = synced.sha256.clone();
    seams
        .db
        .store_upload(owner, &sha256, synced.bytes, now, now + 86_400_000, || {
            seams.objects.place(&synced)
        })
        .unwrap();
    synced.kept();
    sha256
}

/// A `PUT /v1/uploads/sha256:<its hash>` of `bytes`.
pub fn put_upload(bytes: &[u8]) -> Call {
    let sha256 = fitment_core::sha256_hex(bytes);
    Call::new(Method::PUT, &format!("/v1/uploads/sha256:{sha256}"))
        .header("content-type", "application/octet-stream")
        .body(bytes)
}

/// A request body the test feeds chunk by chunk (VT13, VT17, VT24): it
/// ends when its sender drops, and counts the chunks the server took.
pub struct Fed {
    rx: tokio::sync::mpsc::Receiver<axum::body::Bytes>,
    taken: Arc<AtomicUsize>,
}

impl hyper::body::Body for Fed {
    type Data = axum::body::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match self.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(chunk)) => {
                self.taken.fetch_add(1, Ordering::SeqCst);
                std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(chunk))))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

/// A fed body, its sender and its count of chunks taken.
pub fn fed() -> (
    Body,
    tokio::sync::mpsc::Sender<axum::body::Bytes>,
    Arc<AtomicUsize>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let taken = Arc::new(AtomicUsize::new(0));
    let body = Body::new(Fed {
        rx,
        taken: Arc::clone(&taken),
    });
    (body, tx, taken)
}

/// A body of `chunks` chunks of `size` bytes each, made as the server
/// asks for them, counting those it took (VT17's "streamed 100 times past
/// it").
pub struct Endless {
    chunk: axum::body::Bytes,
    left: usize,
    taken: Arc<AtomicUsize>,
}

impl hyper::body::Body for Endless {
    type Data = axum::body::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        if self.left == 0 {
            return std::task::Poll::Ready(None);
        }
        self.left -= 1;
        self.taken.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(self.chunk.clone()))))
    }
}

/// An endless body and its count.
pub fn endless(chunks: usize, size: usize) -> (Body, Arc<AtomicUsize>) {
    let taken = Arc::new(AtomicUsize::new(0));
    let body = Body::new(Endless {
        chunk: axum::body::Bytes::from(vec![b'x'; size]),
        left: chunks,
        taken: Arc::clone(&taken),
    });
    (body, taken)
}

impl TestServer {
    /// Sends `call` in process with `body` instead of its own (a fed or
    /// endless one); the response passes the scan.
    pub async fn send_body(&self, call: Call, body: Body) -> Reply {
        let call = if call.names_credentials() {
            call
        } else {
            let token = self.token().to_owned();
            call.bearer(&token)
        };
        let Transport::InProcess(router) = &self.transport else {
            panic!("send_body is in process only");
        };
        let (parts, _) = call.request(None).into_parts();
        let request = Request::from_parts(parts, body);
        let response = router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let body = body.collect().await.unwrap().to_bytes().to_vec();
        let reply = Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        };
        let problems = scan::response(&reply, &self.dirs.roots());
        assert!(problems.is_empty(), "the response leaks: {problems:?}");
        reply
    }

    /// The files under the state's `tmp/`.
    pub fn temporaries(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.dirs.state.join("tmp"))
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default()
    }

    /// The files under the state's `objects/sha256/`, by name.
    pub fn object_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(fans) = std::fs::read_dir(self.dirs.state.join("objects/sha256")) else {
            return out;
        };
        for fan in fans {
            for file in std::fs::read_dir(fan.unwrap().path()).unwrap() {
                out.push(file.unwrap().file_name().to_string_lossy().into_owned());
            }
        }
        out.sort();
        out
    }

    /// OB2's promise: every object row has its file, of its length and
    /// SHA-256; the problems found.
    pub fn store_problems(&self) -> Vec<String> {
        let seams = self.server().seams();
        let mut problems = Vec::new();
        for (sha256, bytes) in seams.db.objects().unwrap() {
            match seams.objects.read(&sha256, bytes) {
                Ok(read) if u64::try_from(read.len()).unwrap() == bytes => {}
                Ok(_) => problems.push(format!("{sha256}: the wrong length")),
                Err(error) => problems.push(format!("{sha256}: {error}")),
            }
        }
        problems
    }
}

/// A job row of `owner` of `kind` in `state` holding `sha256` as its input,
/// as PR 5's submission writes one (JS11, OB4): straight into `server.db`,
/// since no route makes a job yet. Returns its id.
pub fn seed_job_input(
    server: &TestServer,
    owner: i64,
    sha256: &str,
    kind: &str,
    state: &str,
) -> String {
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM job", [], |r| r.get(0))
        .unwrap();
    let id = format!("j_{:026}", n + 1);
    conn.execute(
        "INSERT INTO job (id, owner, kind, lane, state, request, request_sha256, request_id,
                          created_ms)
         VALUES (?1, ?2, ?3, 'query', ?4, x'7b7d', ?5, 'r_seeded', 0)",
        rusqlite::params![id, owner, kind, state, "0".repeat(64)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO job_input (job, n, sha256) SELECT key, 0, ?2 FROM job WHERE id = ?1",
        rusqlite::params![id, sha256],
    )
    .unwrap();
    id
}

/// Sets a seeded job's state.
pub fn set_job_state(server: &TestServer, id: &str, state: &str) {
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    conn.execute(
        "UPDATE job SET state = ?2 WHERE id = ?1",
        rusqlite::params![id, state],
    )
    .unwrap();
}
