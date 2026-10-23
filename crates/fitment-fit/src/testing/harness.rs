//! The test files' own harness (VT78): libtest prints `running N tests`
//! on stdout before any test runs, which would precede a fake helper's
//! frames, so fitment-fit's process tests are `harness = false` and their
//! `main` is this. It becomes a fake helper when started as one, and else
//! runs the tests it is handed, in parallel, each on its own thread, with
//! libtest's filters (`NAME`, `--exact`, `--skip NAME`), `--list` and
//! `--test-threads`; other libtest flags are accepted and ignored.

// A test runner reports on stdout and exits with its verdict.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::exit,
    clippy::disallowed_methods
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Instant;

/// One test: its name and its body.
pub type Test = (&'static str, fn());

/// Runs `tests` as libtest would, after [`super::fake_main`]: exits 0 if
/// every selected test passes, 101 otherwise.
pub fn harness(tests: &[Test]) {
    super::fake_main();
    let options = Options::parse(std::env::args().skip(1));
    let selected: Vec<Test> = tests
        .iter()
        .copied()
        .filter(|(name, _)| options.selects(name))
        .collect();
    if options.list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        return;
    }
    if options.ignored {
        println!("\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n");
        return;
    }
    let started = Instant::now();
    println!("\nrunning {} tests", selected.len());
    let failed = run(&selected, options.threads);
    let passed = selected.len() - failed.len();
    if !failed.is_empty() {
        println!("\nfailures:");
        for name in &failed {
            println!("    {name}");
        }
    }
    let verdict = if failed.is_empty() { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {verdict}. {passed} passed; {} failed; 0 ignored; {} filtered out; finished in {:.2}s\n",
        failed.len(),
        tests.len() - selected.len(),
        started.elapsed().as_secs_f64()
    );
    if !failed.is_empty() {
        std::process::exit(101);
    }
}

/// Runs `tests` on `threads` threads: the names of those that failed.
fn run(tests: &[Test], threads: usize) -> Vec<&'static str> {
    let queue = Arc::new(Mutex::new(tests.iter().copied().collect::<VecDeque<_>>()));
    let failed = Arc::new(Mutex::new(Vec::new()));
    let workers: Vec<_> = (0..threads.clamp(1, tests.len().max(1)))
        .map(|_| {
            let (queue, failed) = (Arc::clone(&queue), Arc::clone(&failed));
            thread::spawn(move || {
                loop {
                    let next = queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop_front();
                    let Some((name, test)) = next else {
                        break;
                    };
                    let outcome = thread::Builder::new()
                        .name(name.to_owned())
                        .spawn(test)
                        .map(thread::JoinHandle::join);
                    let ok = matches!(outcome, Ok(Ok(())));
                    println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
                    if !ok {
                        failed
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(name);
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    let mut failed = failed
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    failed.sort_unstable();
    failed
}

/// libtest's command line, as far as this harness reads it.
#[derive(Debug, Default)]
struct Options {
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
    list: bool,
    ignored: bool,
    threads: usize,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut out = Self {
            threads: thread::available_parallelism().map_or(4, std::num::NonZero::get),
            ..Self::default()
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--exact" => out.exact = true,
                "--list" => out.list = true,
                "--ignored" => out.ignored = true,
                "--skip" => out.skips.extend(args.next()),
                "--test-threads" => {
                    out.threads = args
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(out.threads);
                }
                "--color" | "--format" | "--logfile" | "-Z" | "--shuffle-seed" => {
                    let _ = args.next();
                }
                other if other.starts_with("--test-threads=") => {
                    out.threads = other["--test-threads=".len()..]
                        .parse()
                        .unwrap_or(out.threads);
                }
                other if other.starts_with('-') => {}
                other => out.filters.push(other.to_owned()),
            }
        }
        out
    }

    fn selects(&self, name: &str) -> bool {
        let matches = |f: &String| {
            if self.exact {
                name == f
            } else {
                name.contains(f.as_str())
            }
        };
        (self.filters.is_empty() || self.filters.iter().any(matches))
            && !self.skips.iter().any(|s| name.contains(s.as_str()))
    }
}
