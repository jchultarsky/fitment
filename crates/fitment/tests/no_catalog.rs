//! A build without the `catalog` feature (`docs/research/m5-catalog.md`,
//! CL8, CT20): `fitment catalog` exits 2 and says why, `--help` included;
//! so does `fitment match --catalog`, before any file is read.

#![cfg(not(feature = "catalog"))]

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn a_build_without_the_catalog_says_so() {
    for args in [&["-C", "x", "list"][..], &["--help"][..]] {
        Command::cargo_bin("fitment")
            .unwrap()
            .arg("catalog")
            .args(args)
            .assert()
            .code(2)
            .stderr(contains(
                "this build has no catalog (cargo feature `catalog`)",
            ));
    }
    Command::cargo_bin("fitment")
        .unwrap()
        .args(["match", "nowhere.json", "-C", "x"])
        .assert()
        .code(2)
        .stderr(contains(
            "this build has no catalog (cargo feature `catalog`)",
        ));
}
