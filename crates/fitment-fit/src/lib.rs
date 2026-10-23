#![doc = include_str!("../README.md")]
#![deny(
    clippy::dbg_macro,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]

mod dir;
mod exit;
pub mod frame;
mod nonce;
mod process;
mod protocol;
mod session;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod testing;

pub use dir::{
    FitDir, HelperFiles, LOCK, MANIFEST, MANIFEST_FORMAT, MANIFEST_FORMAT_VERSION, Manifest,
    PrepareError, SCRIPT, TMP,
};
pub use session::{
    Handshake, HelperError, HelperOptions, HelperReport, Limit, OcctHelper, OcctSession,
    SessionError,
};

/// The helper's protocol (FH6, FC8): it rises with any change to a frame
/// or an operation.
pub const FIT_PROTOCOL: u32 = 1;

/// The kernel a fit is decided for, as `fitment.match` v2 names it (FM2).
pub const KERNEL: &str = "open_cascade";

/// Its exact version (FM2).
pub const KERNEL_VERSION: &str = "8.0.1";

/// The kernel as the helper's handshake states it (FH3).
pub const KERNEL_NAME: &str = "Open CASCADE 8.0.1";

/// The binding the helper imports, as its handshake states it (FH3); the
/// oracle's pin (`tools/verify-occt.py`) is the same.
pub const BINDING: &str = "cadquery-ocp-novtk 8.0.1.1.0";

/// The helper's Python series (FH3, FG2's `requires-python`).
pub const PYTHON: &str = "3.13";

#[cfg(test)]
mod tests {
    use super::*;

    /// FC8: the protocol and the fit's rules are new at 1; the match rules
    /// and v1's format are unchanged, and a run with a verifier writes v2.
    #[test]
    fn the_versions_are_fc8_s() {
        assert_eq!(FIT_PROTOCOL, 1);
        assert_eq!(fitment_core::fit::FIT_RULES_VERSION, 1);
        assert_eq!(fitment_core::MATCH_RULES_VERSION, 1);
        assert_eq!(fitment_core::MATCH_FORMAT_VERSION, 1);
        assert_eq!(fitment_core::MATCH_FORMAT_VERSION_FIT, 2);
        assert_eq!(
            (KERNEL, KERNEL_VERSION, KERNEL_NAME, BINDING, PYTHON),
            (
                "open_cascade",
                "8.0.1",
                "Open CASCADE 8.0.1",
                "cadquery-ocp-novtk 8.0.1.1.0",
                "3.13"
            )
        );
    }
}
