//! VT26's route target (docs/research/m6-api.md, M6-42): arbitrary bytes
//! read as a request (a line `METHOD PATH`, headers, a blank line, the
//! body: VT6's goldens' format, so their requests are the seed corpus)
//! through `Server::router().oneshot` on a current-thread runtime, over a
//! small catalog of two committed unit parts. No panic; no `5xx` but the
//! declared load refusals; every error a valid `fitment.error` document
//! (`fitment_server::fuzzing`).
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use fitment_server::fuzzing::Target;
use libfuzzer_sys::fuzz_target;

/// Two committed unit parts the catalog holds.
const PARTS: [(&str, &[u8]); 2] = [
    (
        "p01_through.stp",
        include_bytes!("../../crates/fitment-core/tests/data/p01_through.stp"),
    ),
    (
        "p09_pin.stp",
        include_bytes!("../../crates/fitment-core/tests/data/p09_pin.stp"),
    ),
];

/// The target, made once a process: a catalog of [`PARTS`] and a state
/// directory under the system's temporary directory.
fn target() -> &'static Target {
    static TARGET: OnceLock<Target> = OnceLock::new();
    TARGET.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("fitment-fuzz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (catalog, parts) = (root.join("catalog"), root.join("parts"));
        std::fs::create_dir_all(&parts).expect("a directory for the parts");
        let (mut made, _) =
            fitment_catalog::Catalog::init(&catalog, fitment_catalog::Options::default())
                .expect("a catalog");
        let files: Vec<PathBuf> = PARTS
            .iter()
            .map(|(name, bytes)| {
                let path = parts.join(name);
                std::fs::write(&path, bytes).expect("a part written");
                path
            })
            .collect();
        made.ingest(&files, None).expect("the parts ingested");
        drop(made);
        Target::new(&catalog, &root.join("state"))
    })
}

fuzz_target!(|input: &[u8]| {
    let _ = target().route(input);
});
