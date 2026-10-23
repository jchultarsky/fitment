//! FG2: the helper's script and its lock, as this build embeds them, with
//! their SHA-256s. The constants are stated, not computed by a build
//! script; `embedded_hashes_are_the_files` recomputes them, so a stale one
//! fails CI. The script is fitment's own MIT text: it imports Open CASCADE
//! at run time and contains none of it (FG5). Both files sit inside the
//! package (`helper/`), so `include_bytes!` survives `cargo package` (FG1).

use crate::dir::HelperFiles;

/// The helper, `helper/fit_helper.py`: a uv script whose inline metadata
/// pins `cadquery-ocp-novtk==8.0.1.1.0` and `requires-python = "==3.13.*"`.
pub const SCRIPT_BYTES: &[u8] = include_bytes!("../helper/fit_helper.py");

/// Its lock, `helper/fit_helper.py.lock` (`uv lock --script`), every wheel
/// with its hash.
pub const LOCK_BYTES: &[u8] = include_bytes!("../helper/fit_helper.py.lock");

/// The script's SHA-256.
pub const SCRIPT_SHA256: &str = "4d60f166cc640ab25300e541f8268d0bc16361a2ac8b11dfc5523d29d621a6ef";

/// The lock's SHA-256.
pub const LOCK_SHA256: &str = "6363b6fdbdbf0eb87ae8a4929b13cc241744e67866b566a0b6262848cab5d432";

impl HelperFiles {
    /// The script and lock this build embeds (FG2): what `fitment fit
    /// setup` writes, and what every spawn checks on disk (FL6).
    pub fn embedded() -> Self {
        Self::new(SCRIPT_BYTES, LOCK_BYTES)
    }
}
