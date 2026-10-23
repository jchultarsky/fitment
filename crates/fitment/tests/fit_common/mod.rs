//! What the corpus's fit suites share (`docs/research/m7-fit.md`, VT58,
//! VT59): where the real helper's fit directory is, and when they run.
//!
//! They need uv and the real helper: they run when `FITMENT_FIT_DIR` names
//! a fit directory `fitment fit setup` made, or when `FITMENT_FIT_KERNEL=1`
//! or `FITMENT_REQUIRE_FIT=1` is set (setup then runs into Cargo's target
//! directory), in a release build, and say they are skipped otherwise.
//! `FITMENT_REQUIRE_FIT=1` makes a missing corpus or a debug build a
//! failure. On Windows the helper is refused until VT77, the weekly Windows
//! corpus run, is green (M7-50); that run sets
//! `FITMENT_FIT_WINDOWS_PROOF=1`, which lets the real helper through in
//! these test builds alone.

#![allow(dead_code)] // each suite uses its own part of it

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use assert_cmd::Command;
use fitment_corpus::Corpus;
use fitment_fit::HelperOptions;

/// True when this run asks for the real helper.
pub fn asked() -> bool {
    let on = |name: &str| std::env::var_os(name).is_some_and(|v| v == "1");
    std::env::var_os("FITMENT_FIT_DIR").is_some()
        || on("FITMENT_FIT_KERNEL")
        || on("FITMENT_REQUIRE_FIT")
}

/// True when a skip is a failure (CI's `fit` job).
pub fn required() -> bool {
    std::env::var_os("FITMENT_REQUIRE_FIT").is_some_and(|v| v == "1")
}

/// The fit directory: `FITMENT_FIT_DIR`, or one `fitment fit setup` makes
/// once per test binary.
pub fn fit_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if !asked() {
            return None;
        }
        if let Some(dir) = std::env::var_os("FITMENT_FIT_DIR") {
            return Some(PathBuf::from(dir));
        }
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fit-setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let out = Command::cargo_bin("fitment")
            .unwrap()
            .current_dir(&root)
            .args(["fit", "setup", "--fit-dir", "fit"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "fitment fit setup: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(root.join("fit"))
    })
    .as_ref()
}

/// The fit directory, if this run asks for the real helper in a release
/// build; `None` with a note otherwise.
pub fn kernel() -> Option<&'static PathBuf> {
    if !asked() {
        eprintln!(
            "skipped: set FITMENT_FIT_DIR or FITMENT_FIT_KERNEL=1 to run the real helper (uv)"
        );
        return None;
    }
    if cfg!(debug_assertions) {
        assert!(!required(), "FITMENT_REQUIRE_FIT=1 needs a release build");
        eprintln!("skipped: a debug build; run with --release");
        return None;
    }
    fit_dir()
}

/// The corpus and the fit directory, or `None` with a note.
pub fn inputs() -> Option<(&'static Corpus, &'static PathBuf)> {
    static CORPUS: OnceLock<Option<Corpus>> = OnceLock::new();
    let dir = kernel()?;
    let corpus = CORPUS
        .get_or_init(|| Corpus::at(&Corpus::default_dir(), required()))
        .as_ref()?;
    Some((corpus, dir))
}

/// VT77's weekly Windows run (`FITMENT_FIT_WINDOWS_PROOF=1`): the real
/// helper let through on Windows, in this test build only, to prove the
/// kernel there (M7-50).
pub fn windows_proof() -> bool {
    cfg!(windows) && std::env::var_os("FITMENT_FIT_WINDOWS_PROOF").is_some_and(|v| v == "1")
}

/// True where the helper is refused until VT77 is green (M7-50), but in
/// VT77's own run.
pub fn refused_here() -> bool {
    cfg!(windows) && !windows_proof()
}

/// `options`, with VT77's proof where its run asks for it.
pub fn proved(options: HelperOptions) -> HelperOptions {
    if windows_proof() {
        options.with_windows_proof()
    } else {
        options
    }
}
