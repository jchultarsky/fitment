//! What the CLI tests share.

/// The comparison of features against a test manifest, shared with
/// fitment-core's tests.
#[path = "../../../fitment-core/tests/support/expect.rs"]
#[allow(dead_code)] // each test binary uses part of it
pub mod expect;

/// The comparison of an assembly against its test case, shared with
/// fitment-core's tests.
#[path = "../../../fitment-core/tests/support/assembly.rs"]
#[allow(dead_code)] // each test binary uses part of it
pub mod assembly;

/// The comparison of a socket with its `_rot` and `_tgt` variants'.
#[allow(dead_code)] // each test binary uses part of it
pub mod variants;

/// Why fitment-core cannot read back the document `fitment features`
/// printed, if it cannot: every document it writes must keep the rules
/// `from_json` checks.
pub fn reads_back(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    fitment_core::FeaturesDocument::from_json(&text)
        .err()
        .map(|error| format!("its document does not read back: {error}"))
}
