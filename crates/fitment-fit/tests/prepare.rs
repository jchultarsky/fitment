//! VT57 (`docs/research/m7-fit.md`, FL2, FL6, FY2, FY5, FG1): the fit
//! directory and its manifest. No manifest, one naming a missing
//! interpreter or site or another binding, a tampered script or lock, a
//! symbolic link and an unwritable `tmp/` are each refused with their
//! sentence, before every spawn; a helper is spawned from the manifest
//! alone, with nothing inherited. `embedded_hashes_are_the_files` (FG2)
//! comes with the embedded script, in PR 2b.

mod common;

use std::path::Path;

use common::{S01, code, fast, judge, root, s01};
use fitment_core::fit::FitCode;
use fitment_fit::testing::{FAKE_PYTHON, Fake, Replaying, Scenario};
use fitment_fit::{
    FitDir, HelperError, HelperFiles, LOCK, MANIFEST, Manifest, OcctHelper, OcctSession,
    PrepareError, SCRIPT, TMP,
};
use serde_json::Value;

fn main() {
    fitment_fit::harness![
        no_manifest_is_refused,
        a_manifest_naming_what_is_not_there_is_refused,
        a_manifest_for_another_build_is_refused,
        a_tampered_script_or_lock_is_refused_before_every_spawn,
        a_symbolic_link_is_refused,
        an_unwritable_tmp_is_refused,
        the_fit_directory_is_private_and_written_whole,
        the_helper_is_spawned_from_the_manifest_with_nothing_inherited,
        the_crate_is_pure_rust_on_four_crates,
        a_fit_directory_other_users_can_write_is_refused,
        stale_helper_directories_never_block_a_spawn,
    ];
}

/// `open`'s refusal: its sentence names no path and says what to run; its
/// message names the path.
fn refused(dir: &Path, files: &HelperFiles) -> PrepareError {
    let error = OcctHelper::open(dir, files.clone(), fast()).unwrap_err();
    let sentence = error.sentence();
    assert!(!sentence.contains(dir.to_str().unwrap()), "FV9: {sentence}");
    error
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).unwrap()).unwrap()
}

fn rewrite(dir: &Path, edit: impl FnOnce(&mut Value)) {
    let mut value = manifest(dir);
    edit(&mut value);
    std::fs::write(dir.join(MANIFEST), value.to_string()).unwrap();
}

/// FL2: a fit directory `fitment fit setup` never finished has no
/// manifest: refused, "run `fitment fit setup`".
fn no_manifest_is_refused() {
    let root = root("no-manifest");
    let fake = Fake::install(&root, Scenario::Replay);
    std::fs::remove_file(fake.dir.join(MANIFEST)).unwrap();
    let error = refused(&fake.dir, &fake.files);
    assert!(
        matches!(error, PrepareError::NoManifest { .. }),
        "{error:?}"
    );
    assert!(error.sentence().contains("run `fitment fit setup`"));
    assert!(error.to_string().contains(fake.dir.to_str().unwrap()));
    std::fs::write(fake.dir.join(MANIFEST), "{").unwrap();
    assert!(matches!(
        refused(&fake.dir, &fake.files),
        PrepareError::BadManifest { .. }
    ));
}

/// FL2: the interpreter and the site the manifest names must exist.
fn a_manifest_naming_what_is_not_there_is_refused() {
    let root = root("missing");
    let fake = Fake::install(&root, Scenario::Replay);
    let gone = root.join("python3.13");
    rewrite(&fake.dir, |m| {
        m["interpreter"] = Value::from(gone.to_str().unwrap());
    });
    let error = refused(&fake.dir, &fake.files);
    assert!(
        matches!(
            error,
            PrepareError::Missing {
                what: "interpreter",
                ..
            }
        ),
        "{error:?}"
    );
    let fake = Fake::install(&root.join("site"), Scenario::Replay);
    rewrite(&fake.dir, |m| {
        m["site"] = Value::from(root.join("nowhere").to_str().unwrap());
    });
    let error = refused(&fake.dir, &fake.files);
    assert!(
        matches!(error, PrepareError::Missing { what: "site", .. }),
        "{error:?}"
    );
    let fake = Fake::install(&root.join("relative"), Scenario::Replay);
    rewrite(&fake.dir, |m| {
        m["site"] = Value::from("site-packages");
    });
    assert!(matches!(
        refused(&fake.dir, &fake.files),
        PrepareError::BadManifest { .. }
    ));
}

/// A named edit of a manifest.
type Edit = (&'static str, fn(&mut Value));

/// FL2, FH3: a manifest of another binding, kernel, Python, format, or
/// script, or with a key this build does not know, is refused.
fn a_manifest_for_another_build_is_refused() {
    let root = root("another");
    let edits: [Edit; 7] = [
        ("binding", |m| {
            m["binding"] = Value::from("cadquery-ocp-novtk 7.8.1.1.0");
        }),
        ("kernel", |m| {
            m["kernel"] = Value::from("Open CASCADE 7.8.1");
        }),
        ("Python", |m| m["python"] = Value::from("3.12.4")),
        ("Python 3.130", |m| m["python"] = Value::from("3.130.0")),
        ("format", |m| m["format_version"] = Value::from(2)),
        ("script", |m| {
            m["helper_sha256"] = Value::from("0".repeat(64));
        }),
        ("unknown key", |m| m["uv"] = Value::from("0.9")),
    ];
    for (i, (what, edit)) in edits.into_iter().enumerate() {
        let fake = Fake::install(&root.join(i.to_string()), Scenario::Replay);
        rewrite(&fake.dir, edit);
        let error = refused(&fake.dir, &fake.files);
        assert!(
            error.sentence().contains("run `fitment fit setup`"),
            "{what}: {error}"
        );
    }
}

/// FL6, FY5: the script or lock on disk not this build's is refused when
/// the helper is opened, and again before every spawn: a fit after the
/// edit is `helper_unavailable`.
fn a_tampered_script_or_lock_is_refused_before_every_spawn() {
    let root = root("tampered");
    for name in [SCRIPT, LOCK] {
        let fake = Fake::install(&root.join(name), Scenario::Replay);
        let path = fake.dir.join(name);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, [original.as_slice(), b"\n# edited"].concat()).unwrap();
        let error = refused(&fake.dir, &fake.files);
        assert!(
            matches!(error, PrepareError::Changed { .. }),
            "{name}: {error:?}"
        );
        std::fs::write(&path, &original).unwrap();
        let helper = fake.helper(fast());
        std::fs::write(&path, [original.as_slice(), b"\n# edited"].concat()).unwrap();
        assert!(
            matches!(helper.check(), Err(HelperError::Failed(_))),
            "{name}"
        );
        let document = s01();
        let session = OcctSession::open(&helper, S01.to_vec(), &document).unwrap();
        let result = judge(&Replaying::new(&session, &fake));
        assert_eq!(code(&result), Some(FitCode::HelperUnavailable), "{name}");
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            refused(&fake.dir, &fake.files),
            PrepareError::Missing { .. }
        ));
    }
}

/// FL6: the fit directory, its `tmp/` and its files are never symbolic
/// links (Unix: Windows' need a privilege to make).
fn a_symbolic_link_is_refused() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let root = root("symlink");
        let fake = Fake::install(&root.join("real"), Scenario::Replay);
        let link = root.join("link");
        symlink(&fake.dir, &link).unwrap();
        assert!(matches!(
            refused(&link, &fake.files),
            PrepareError::SymbolicLink { .. }
        ));
        for name in [TMP, SCRIPT, MANIFEST] {
            let fake = Fake::install(&root.join(name), Scenario::Replay);
            let path = fake.dir.join(name);
            let moved = root.join(format!("{name}.moved"));
            std::fs::rename(&path, &moved).unwrap();
            symlink(&moved, &path).unwrap();
            let error = refused(&fake.dir, &fake.files);
            assert!(
                matches!(error, PrepareError::SymbolicLink { .. }),
                "{name}: {error:?}"
            );
        }
    }
}

/// FL6: `tmp/` must be writable, each helper making its own directory
/// there (Unix: Windows has no mode that stops it).
fn an_unwritable_tmp_is_refused() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = root("unwritable");
        let fake = Fake::install(&root, Scenario::Replay);
        let tmp = fake.dir.join(TMP);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o500)).unwrap();
        let error = refused(&fake.dir, &fake.files);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(error, PrepareError::TmpUnwritable { .. }),
            "{error:?}"
        );
        assert_eq!(
            error.sentence(),
            "the helpers' payload directory is not writable"
        );
    }
}

/// FL6: the directory and `tmp/` are made 0700; the files are written
/// whole, with nothing left of their temporary names; the manifest reads
/// back as written.
fn the_fit_directory_is_private_and_written_whole() {
    let root = root("private");
    let fake = Fake::install(&root, Scenario::Replay);
    #[cfg(unix)]
    for dir in [fake.dir.clone(), fake.dir.join(TMP)] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
    }
    let mut names: Vec<String> = std::fs::read_dir(&fake.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, [SCRIPT, LOCK, MANIFEST, TMP]);
    #[cfg(unix)]
    for name in [SCRIPT, LOCK, MANIFEST] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(fake.dir.join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name}: whatever the umask");
    }
    let dir = FitDir::open(&fake.dir).unwrap();
    let written = Manifest::new(
        std::env::current_exe().unwrap(),
        root.join("site-packages"),
        FAKE_PYTHON,
        &fake.files,
    )
    .unwrap();
    assert_eq!(dir.read_manifest().unwrap(), written);
    assert_eq!(dir.check(&fake.files).unwrap(), written);
    // Creating it again changes nothing.
    FitDir::create(&fake.dir).unwrap();
    assert_eq!(dir.check(&fake.files).unwrap(), written);
}

/// FL2, FY2: the helper is the manifest's interpreter with `-I`, the
/// script and `--site`, and nothing else: no environment but Windows'
/// `SYSTEMROOT`, its own working directory under `tmp/` (removed with
/// it), and no descriptor but its three pipes.
fn the_helper_is_spawned_from_the_manifest_with_nothing_inherited() {
    let root = root("spawn");
    let fake = Fake::install(&root, Scenario::Report);
    let helper = fake.helper(fast());
    // What this process was itself started with: a CI runner can hand its
    // processes descriptors without close-on-exec (Ubuntu's 142 and 145),
    // which std passes on and cannot close without `unsafe`; fitment adds
    // none of its own.
    let inherited = open_fds();
    // A file this process holds open is not the helper's.
    let held = std::fs::File::open(fake.dir.join(MANIFEST)).unwrap();
    let held_fd = held_fd(&held);
    let handshake = helper.check().unwrap();
    drop(held);
    let report: Value = serde_json::from_str(
        &fake
            .read_state(&format!("report-{}.json", handshake.pid))
            .expect("the helper's report"),
    )
    .unwrap();
    let script = fake.dir.join(SCRIPT);
    let site = root.join("site-packages");
    assert_eq!(
        report["args"],
        serde_json::json!([
            "-I",
            script.to_str().unwrap(),
            "--site",
            site.to_str().unwrap()
        ])
    );
    let env: Vec<&str> = report["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    if cfg!(windows) {
        assert!(
            env.iter().all(|k| k.eq_ignore_ascii_case("SYSTEMROOT")),
            "{env:?}"
        );
    } else {
        assert!(env.is_empty(), "{env:?}");
    }
    let cwd = Path::new(report["cwd"].as_str().unwrap()).to_owned();
    let tmp = fake.dir.join(TMP).canonicalize().unwrap();
    assert!(
        cwd.starts_with(&tmp) || cwd.starts_with(fake.dir.join(TMP)),
        "{}",
        cwd.display()
    );
    assert!(!cwd.exists(), "its directory is removed with it");
    if cfg!(unix) {
        // Its three pipes, and the directory listing them.
        let mut fds: Vec<u32> = report["fds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse().unwrap())
            .collect();
        fds.sort_unstable();
        // Its three pipes and the listing's own descriptor, and nothing
        // else but what this process was started with.
        for fd in [0, 1, 2, 3] {
            assert!(fds.contains(&fd), "{fds:?}");
        }
        let extra: Vec<u32> = fds
            .iter()
            .copied()
            .filter(|fd| *fd > 3 && !inherited.contains(fd))
            .collect();
        assert!(
            extra.is_empty(),
            "fitment's own, inherited: {extra:?} of {fds:?}"
        );
        assert!(!fds.contains(&held_fd), "the held file: {fds:?}");
    }
    assert_eq!(handshake.python, FAKE_PYTHON);
}

/// The descriptors this process has open (Unix; none elsewhere).
fn open_fds() -> Vec<u32> {
    std::fs::read_dir("/dev/fd")
        .map(|d| {
            d.filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The descriptor `file` holds (Unix; 0 elsewhere).
fn held_fd(file: &std::fs::File) -> u32 {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        u32::try_from(file.as_raw_fd()).unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        0
    }
}

/// FG1: fitment-fit depends on fitment-core, serde, `serde_json` and
/// thiserror alone, inherits the workspace's lints (`unsafe_code =
/// "forbid"`), and packages its sources, its helper, README and LICENSE.
fn the_crate_is_pure_rust_on_four_crates() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(here.join("Cargo.toml")).unwrap();
    let section = |name: &str| -> Vec<String> {
        manifest
            .split(&format!("\n[{name}]\n"))
            .nth(1)
            .unwrap_or_default()
            .split("\n[")
            .next()
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with('#') && l.contains('='))
            .map(|l| l.split('=').next().unwrap().trim().to_owned())
            .collect()
    };
    assert_eq!(
        section("dependencies"),
        ["fitment-core", "serde", "serde_json", "thiserror"]
    );
    assert_eq!(section("lints"), ["workspace"]);
    assert!(manifest.contains(r#"include = ["src/**", "helper/**", "README.md", "LICENSE"]"#));
    let workspace = std::fs::read_to_string(here.join("../../Cargo.toml")).unwrap();
    assert!(workspace.contains("[workspace.lints.rust]\nunsafe_code = \"forbid\""));
}

/// The reviews of PR 2a (P4): a fit directory, `tmp/` or file another
/// user could write would let them run any program as this one: each is
/// refused when the helper is opened and again before every spawn (Unix;
/// on Windows the directory's ACLs are not read, and FY5's trust boundary,
/// the user's account, holds). An interpreter that is not the one the
/// manifest hashed is refused too.
fn a_fit_directory_other_users_can_write_is_refused() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = root("others");
        for (name, mode) in [
            ("", 0o777),
            (TMP, 0o777),
            (MANIFEST, 0o666),
            (SCRIPT, 0o666),
            (LOCK, 0o660),
        ] {
            let fake = Fake::install(&root.join(format!("{name}-{mode:o}")), Scenario::Replay);
            let path = if name.is_empty() {
                fake.dir.clone()
            } else {
                fake.dir.join(name)
            };
            let kept = std::fs::metadata(&path).unwrap().permissions();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = refused(&fake.dir, &fake.files);
            assert!(
                matches!(error, PrepareError::NotPrivate { .. }),
                "{name} {mode:o}: {error:?}"
            );
            assert!(error.sentence().contains("group- or world-writable"));
            // Opened while private, then opened up: the next spawn refuses.
            std::fs::set_permissions(&path, kept.clone()).unwrap();
            let helper = fake.helper(fast());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let Err(HelperError::Failed(report)) = helper.check() else {
                panic!("{name} {mode:o}: spawned");
            };
            assert!(
                report.why.contains("group- or world-writable"),
                "{report:?}"
            );
            std::fs::set_permissions(&path, kept).unwrap();
        }
    }
    let root = root("interpreter");
    let fake = Fake::install(&root, Scenario::Replay);
    rewrite(&fake.dir, |m| {
        m["interpreter_sha256"] = Value::from("0".repeat(64));
    });
    let error = refused(&fake.dir, &fake.files);
    assert!(
        matches!(
            error,
            PrepareError::Changed {
                what: "interpreter",
                ..
            }
        ),
        "{error:?}"
    );
}

/// The reviews of PR 2a (P3): directories a crashed run left under `tmp/`
/// with this process's names never block a spawn, and those older than a
/// day are swept when a helper is opened.
fn stale_helper_directories_never_block_a_spawn() {
    let root = root("stale-dirs");
    let fake = Fake::install(&root, Scenario::Replay);
    let tmp = fake.dir.join(TMP);
    let next = fitment_fit::testing::next_helper_dir();
    for n in next..next + 300 {
        std::fs::create_dir(tmp.join(format!("helper-{}-{n}", std::process::id()))).unwrap();
    }
    let helper = fake.helper(fast());
    assert!(helper.check().is_ok());
    #[cfg(unix)]
    {
        let old = tmp.join("helper-1-1");
        let young = tmp.join("helper-1-2");
        for dir in [&old, &young] {
            std::fs::create_dir(dir).unwrap();
        }
        let two_days = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600);
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(two_days)
            .unwrap();
        let _helper = fake.helper(fast());
        assert!(!old.exists(), "swept");
        assert!(young.exists(), "kept");
    }
}
