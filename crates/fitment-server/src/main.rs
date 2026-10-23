//! `fitment-server` (docs/research/m6-api.md, SV2, OP1 to OP5): the only
//! place that reads the process's arguments, installs the log subscriber
//! or exits. Stdout carries one line, `fitment-server: listening on ADDR`,
//! once the listener is bound (M6-54); logs go to stderr.
//!
//! Exit codes: 0 after a graceful stop; 2 for a configuration, a state
//! directory or a lease the operator must change; 3 for a catalog or
//! `server.db` of a newer schema; 1 for I/O, a port in use among them.
//!
//! `admin` holds AU9's offline commands: `bootstrap`, `user` and `token`.
//! A secret they make goes to stdout once, alone on its line; what they
//! say goes to stderr and never holds one.

use std::io::{BufRead as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use fitment_server::admin::{Admin, AdminError};
use fitment_server::auth::{Minted, Role};
use fitment_server::{Config, Flags, Server};

#[derive(Parser)]
#[command(name = "fitment-server", version, about = "fitment's REST API")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serves the API until SIGTERM or Ctrl-C.
    Serve(ConfigArgs),
    /// Loads and checks the configuration, opens the catalog and reads
    /// server.db's header, prints the effective configuration; exits 0 or 2
    /// (OP4).
    CheckConfig(ConfigArgs),
    /// Asks the configured listener's /v1/health; exits 0 if it answers 204
    /// (OP5: the image's HEALTHCHECK).
    Healthcheck(ConfigArgs),
    /// Users and tokens, offline, on server.db directly, while the server
    /// runs or not (AU9).
    #[command(subcommand)]
    Admin(AdminCommand),
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Makes the first admin and prints a new admin token to stdout, once;
    /// refused if an admin exists.
    Bootstrap {
        #[command(flatten)]
        at: StateArgs,
        /// The admin's username.
        #[arg(long)]
        username: String,
        /// Also sets a password, read from stdin.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Users.
    #[command(subcommand)]
    User(UserCommand),
    /// Tokens.
    #[command(subcommand)]
    Token(TokenCommand),
}

#[derive(Subcommand)]
enum UserCommand {
    /// Makes a user (token-only without --password-stdin); prints its id.
    Add {
        #[command(flatten)]
        at: StateArgs,
        #[arg(long)]
        username: String,
        /// reader, writer or admin.
        #[arg(long, value_parser = role, default_value = "reader")]
        role: Role,
        /// Sets a password, read from stdin.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Lists the users: id, username, role, enabled or disabled.
    List {
        #[command(flatten)]
        at: StateArgs,
    },
    /// Sets a user's role or disables or enables it.
    Set {
        #[command(flatten)]
        at: StateArgs,
        #[arg(long)]
        username: String,
        #[arg(long, value_parser = role)]
        role: Option<Role>,
        /// true or false.
        #[arg(long)]
        disabled: Option<bool>,
    },
    /// Sets a new password, read from stdin, and revokes every token of
    /// the user.
    ResetPassword {
        #[command(flatten)]
        at: StateArgs,
        #[arg(long)]
        username: String,
        /// Reads the password from stdin (required).
        #[arg(long, required = true)]
        password_stdin: bool,
    },
}

#[derive(Subcommand)]
enum TokenCommand {
    /// Mints a token for a user and prints it to stdout, once.
    Mint {
        #[command(flatten)]
        at: StateArgs,
        #[arg(long)]
        username: String,
        /// Its name.
        #[arg(long, default_value = "admin")]
        name: String,
        /// At most the user's role; the user's when not given.
        #[arg(long, value_parser = role)]
        role: Option<Role>,
        /// Its lifetime in days (`auth.token_default_days` when not given).
        #[arg(long)]
        days: Option<u32>,
    },
    /// Lists a user's tokens: id, name, role, hint, expiry, revocation.
    List {
        #[command(flatten)]
        at: StateArgs,
        #[arg(long)]
        username: String,
    },
    /// Revokes a token and every token minted from it.
    Revoke {
        #[command(flatten)]
        at: StateArgs,
        /// The token's id, t_….
        #[arg(long)]
        id: String,
    },
}

/// Where the offline commands work: the state directory, and the
/// configuration naming the pepper (AU1).
#[derive(Args)]
struct StateArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "The configuration file (or FITMENT_SERVER_CONFIG), for auth.pepper_file and [auth]"
    )]
    config: Option<PathBuf>,
    /// The server's state directory (paths.state).
    #[arg(long, value_name = "DIR")]
    state: Option<PathBuf>,
}

fn role(text: &str) -> Result<Role, String> {
    Role::parse(text).ok_or_else(|| "reader, writer or admin".to_owned())
}

/// The only flags (OP2); every other key is the file's or the
/// environment's.
#[derive(Args)]
struct ConfigArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "The configuration file (or FITMENT_SERVER_CONFIG)"
    )]
    config: Option<PathBuf>,
    /// The address to listen on, as 127.0.0.1:8080 (server.listen).
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,
    /// The catalog's directory (paths.catalog).
    #[arg(long, value_name = "DIR")]
    catalog: Option<PathBuf>,
    /// The server's state directory (paths.state).
    #[arg(long, value_name = "DIR")]
    state: Option<PathBuf>,
    /// json or text (log.format).
    #[arg(long, value_name = "FORMAT")]
    log_format: Option<String>,
}

impl ConfigArgs {
    fn flags(&self) -> Flags {
        let mut flags = Flags::default();
        flags.config.clone_from(&self.config);
        flags.listen.clone_from(&self.listen);
        flags.catalog.clone_from(&self.catalog);
        flags.state.clone_from(&self.state);
        flags.log_format.clone_from(&self.log_format);
        flags
    }

    fn load(&self) -> Result<Config, ExitCode> {
        Config::load(&self.flags()).map_err(|e| fail(2, &e.to_string()))
    }
}

fn fail(code: u8, message: &str) -> ExitCode {
    eprintln!("fitment-server: {message}");
    ExitCode::from(code)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => serve(&args),
        Command::CheckConfig(args) => check_config(&args),
        Command::Healthcheck(args) => healthcheck(&args),
        Command::Admin(command) => admin(command),
    }
}

/// AU9's commands: their result, or the error's exit code.
fn admin(command: AdminCommand) -> ExitCode {
    match run_admin(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(error.exit_code(), &error.to_string()),
    }
}

fn open(at: &StateArgs) -> Result<Admin, AdminError> {
    let mut flags = Flags::default();
    flags.config.clone_from(&at.config);
    flags.state.clone_from(&at.state);
    let config = Config::load_offline(&flags).map_err(|e| AdminError::Usage(e.to_string()))?;
    Admin::open(&config)
}

/// The password on stdin, its final line break dropped; refused from a
/// terminal, where it would be echoed and kept in the scrollback (review).
fn password_from_stdin() -> Result<String, AdminError> {
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(AdminError::Usage(
            "--password-stdin reads a pipe, not a terminal: pipe the password in, as \
             `printf '%s' \"$PASSWORD\" | fitment-server admin …`"
                .into(),
        ));
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut text)
        .map_err(|_| AdminError::Usage("the password on stdin is not UTF-8".into()))?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    Ok(text)
}

/// Prints a minted token's secret to stdout, alone and once, and what it
/// is to stderr, without it.
fn print_minted(minted: &Minted, what: &str) {
    println!("{}", minted.secret.expose());
    let _ = std::io::stdout().flush();
    eprintln!(
        "fitment-server: {what}: token {} ({}), expires {}",
        minted.row.id,
        minted.row.role.name(),
        fitment_server::ids::rfc3339(minted.row.expires_ms)
    );
}

fn run_admin(command: AdminCommand) -> Result<(), AdminError> {
    match command {
        AdminCommand::Bootstrap {
            at,
            username,
            password_stdin,
        } => {
            let password = password_stdin.then(password_from_stdin).transpose()?;
            let minted = open(&at)?.bootstrap(&username, password.as_deref())?;
            print_minted(&minted, &format!("admin {username} created"));
        }
        AdminCommand::User(command) => run_user(command)?,
        AdminCommand::Token(command) => run_token(command)?,
    }
    Ok(())
}

fn run_user(command: UserCommand) -> Result<(), AdminError> {
    match command {
        UserCommand::Add {
            at,
            username,
            role,
            password_stdin,
        } => {
            let password = password_stdin.then(password_from_stdin).transpose()?;
            let user = open(&at)?.add_user(&username, role, password.as_deref())?;
            println!("{}", user.id);
        }
        UserCommand::List { at } => {
            for user in open(&at)?.users()? {
                let state = if user.disabled { "disabled" } else { "enabled" };
                println!("{} {} {} {state}", user.id, user.username, user.role.name());
            }
        }
        UserCommand::Set {
            at,
            username,
            role,
            disabled,
        } => {
            let user = open(&at)?.set_user(&username, role, disabled)?;
            eprintln!("fitment-server: {} is {}", user.username, user.role.name());
        }
        UserCommand::ResetPassword { at, username, .. } => {
            let password = password_from_stdin()?;
            let revoked = open(&at)?.reset_password(&username, &password)?;
            eprintln!("fitment-server: password set; {revoked} tokens revoked");
        }
    }
    Ok(())
}

fn run_token(command: TokenCommand) -> Result<(), AdminError> {
    match command {
        TokenCommand::Mint {
            at,
            username,
            name,
            role,
            days,
        } => {
            let minted = open(&at)?.mint(&username, &name, role, days)?;
            print_minted(&minted, &format!("minted for {username}"));
        }
        TokenCommand::List { at, username } => {
            for token in open(&at)?.tokens(&username)? {
                let revoked = token
                    .revoked_ms
                    .map_or_else(|| "live".to_owned(), fitment_server::ids::rfc3339);
                println!(
                    "{} {:?} {} {} {} {revoked}",
                    token.id,
                    token.name,
                    token.role.name(),
                    token.hint,
                    fitment_server::ids::rfc3339(token.expires_ms)
                );
            }
        }
        TokenCommand::Revoke { at, id } => {
            let revoked = open(&at)?.revoke(&id)?;
            eprintln!("fitment-server: {revoked} tokens revoked");
        }
    }
    Ok(())
}

/// OP1: JSON or text on stderr, filtered by `RUST_LOG`, then `log.filter`.
fn install_logs(config: &Config) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(&config.log.filter))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if config.log.format == "text" {
        builder.with_ansi(false).init();
    } else {
        builder.json().with_current_span(true).init();
    }
}

fn serve(args: &ConfigArgs) -> ExitCode {
    let config = match args.load() {
        Ok(config) => config,
        Err(code) => return code,
    };
    install_logs(&config);
    tracing::info!(configuration = %config.to_json(), "starting");
    if !config.ignored.is_empty() {
        tracing::warn!(
            variables = ?config.ignored,
            "ignored: FITMENT_SERVER_* variables that name no key (a key's is FITMENT_SERVER__SECTION__KEY)"
        );
    }
    let jobs = &config.jobs;
    let cpu = usize::try_from(jobs.cpu_budget()).unwrap_or(1);
    let query_slots = usize::try_from(jobs.query_slots()).unwrap_or(1);
    let abandoned = usize::try_from(jobs.abandoned_max).unwrap_or(0);
    let Ok(addr) = config.listen_addr() else {
        return fail(2, "server.listen is not an address");
    };
    let server = match Server::build(config) {
        Ok(server) => server,
        Err(error) => {
            tracing::error!(error = %error, "refused to start");
            return fail(error.exit_code(), &error.to_string());
        }
    };
    // SV3: workers for I/O only; compute is bounded by the jobs' permits.
    // JS8 (PR 8): the threads of jobs given up stay on the blocking pool,
    // `jobs.abandoned_max` at most, so the pool has room for them.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cpu.clamp(1, 4))
        .max_blocking_threads(64 + query_slots + 2 + abandoned)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(1, &format!("cannot start the runtime: {}", error.kind())),
    };
    let code = runtime.block_on(run(server, addr));
    // JS8: a stuck call's thread never returns; the exit does not wait for
    // it (dropping the runtime would, forever).
    runtime.shutdown_timeout(Duration::from_secs(2));
    code
}

async fn run(server: Server, addr: SocketAddr) -> ExitCode {
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(address = %addr, error = %error, "cannot listen");
            return fail(1, &format!("cannot listen on {addr}: {}", error.kind()));
        }
    };
    let bound = listener.local_addr().unwrap_or(addr);
    // SD3 (10), M6-54: the one line on stdout, after everything else.
    println!("fitment-server: listening on {bound}");
    let _ = std::io::stdout().flush();
    tracing::info!(address = %bound, "listening");
    match server.serve(listener, shutdown()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(1, &error.to_string()),
    }
}

/// SD1's triggers: SIGTERM or SIGINT on Unix, Ctrl-C, Ctrl-Break or a
/// close event on Windows; under
/// `test-support`, stdin's end of file too (OP11), the portable trigger
/// for the tests.
async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    // SD1 on Windows: Ctrl-Break and a console's close event, beside
    // Ctrl-C above.
    #[cfg(windows)]
    let terminate = async {
        use tokio::signal::windows::{ctrl_break, ctrl_close};
        match (ctrl_break(), ctrl_close()) {
            (Ok(mut brk), Ok(mut close)) => {
                tokio::select! {
                    _ = brk.recv() => {},
                    _ = close.recv() => {},
                }
            }
            _ => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(any(unix, windows)))]
    let terminate = std::future::pending::<()>();
    #[cfg(feature = "test-support")]
    let eof = async {
        let _ = tokio::task::spawn_blocking(|| {
            let mut sink = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut sink);
        })
        .await;
    };
    #[cfg(not(feature = "test-support"))]
    let eof = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
        () = eof => {},
    }
    tracing::info!("stopping");
}

/// OP4: the configuration loaded and checked, the catalog opened, the
/// nesting checked and `server.db`'s header read, nothing written.
fn check_config(args: &ConfigArgs) -> ExitCode {
    let config = match args.load() {
        Ok(config) => config,
        Err(code) => return code,
    };
    for name in &config.ignored {
        eprintln!(
            "fitment-server: ignored {name}: it names no key (a key's is FITMENT_SERVER__SECTION__KEY)"
        );
    }
    match fitment_server::check(&config) {
        Ok(()) => {
            print!("{}", config.to_json());
            ExitCode::SUCCESS
        }
        Err(error) => fail(error.exit_code(), &error.to_string()),
    }
}

/// OP5: `GET /v1/health` with a std `TcpStream` (distroless has no curl).
fn healthcheck(args: &ConfigArgs) -> ExitCode {
    let config = match args.load() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let Ok(mut addr) = config.listen_addr() else {
        return fail(2, "server.listen is not an address");
    };
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let answered = (|| -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        write!(
            stream,
            "GET /v1/health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )?;
        let mut line = String::new();
        std::io::BufReader::new(stream).read_line(&mut line)?;
        Ok(line.starts_with("HTTP/1.1 204"))
    })();
    match answered {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => fail(1, "the server answered, but not 204"),
        Err(error) => fail(1, &format!("no answer from {addr}: {}", error.kind())),
    }
}
