//! VT26's route target (docs/research/m6-api.md, M6-42): arbitrary bytes
//! read as a request (a line `METHOD PATH`, headers, a blank line, the
//! body: VT6's goldens' format, so their requests are the seed corpus)
//! through `Server::router().oneshot` on a current-thread runtime, over
//! the goldens' world (`fitment_server::fuzzing::Target`, XH14: their
//! people, uploads, assembly, socket, jobs, match and catalog entries).
//! No panic; no `5xx` but the declared load refusals; every error a valid
//! `fitment.error` document.
#![no_main]

use std::sync::OnceLock;

use fitment_server::fuzzing::Target;
use libfuzzer_sys::fuzz_target;

/// The target, made once a process: an empty catalog, which the target
/// fills through the API, and a state directory, under the system's
/// temporary directory.
fn target() -> &'static Target {
    static TARGET: OnceLock<Target> = OnceLock::new();
    TARGET.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("fitment-fuzz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let catalog = root.join("catalog");
        fitment_catalog::Catalog::init(&catalog, fitment_catalog::Options::default())
            .expect("a catalog");
        Target::new(&catalog, &root.join("state"))
    })
}

fuzz_target!(|input: &[u8]| {
    let _ = target().route(input);
});
