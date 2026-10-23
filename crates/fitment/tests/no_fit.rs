//! A build without the `fit` feature (`docs/research/m7-fit.md`, FG3):
//! `fitment fit` exits 2 and says the build has no fit verifier, `--help`
//! included; so does `fitment match --fit`, before any file is read.

#![cfg(not(feature = "fit"))]

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn a_build_without_the_fit_verifier_says_so() {
    for args in [&["setup"][..], &["--help"][..]] {
        Command::cargo_bin("fitment")
            .unwrap()
            .arg("fit")
            .args(args)
            .assert()
            .code(2)
            .stderr(contains(
                "this build has no fit verifier (cargo feature `fit`)",
            ));
    }
    let socket = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/s01_bolted--BRACKET.rules5.json");
    Command::cargo_bin("fitment")
        .unwrap()
        .args([
            "match",
            socket.to_str().unwrap(),
            "nowhere.stp",
            "--fit",
            "nowhere.stp",
        ])
        .assert()
        .code(2)
        .stderr(contains(
            "this build has no fit verifier (cargo feature `fit`)",
        ));
}
