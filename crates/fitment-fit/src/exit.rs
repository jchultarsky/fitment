//! FV8: how a helper that fitment did not kill ended. On Unix by the
//! signal that ended it: `SIGSEGV`, `SIGBUS`, `SIGABRT` and `SIGFPE` are
//! crashes (retried once, then `kernel_crashed`, the run complete);
//! `SIGXCPU`, `SIGXFSZ` and a `SIGKILL` fitment did not send (an
//! out-of-memory killer's mark) are resource failures (`resource_exhausted`,
//! the run incomplete). On Windows by its exit code: one in the
//! `0xC0000000` range (an NTSTATUS error: an access violation, a stack
//! overflow, a fast fail) is a crash. Anything else, an exit with a code
//! included, is a protocol failure: the helper ended without answering.

use std::process::ExitStatus;

/// How a helper ended, for FV8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitClass {
    /// The kernel crashed: retried once in a fresh helper.
    Crash,
    /// A resource limit ended it: the run is incomplete.
    Resource,
    /// It ended some other way: a protocol failure.
    Other,
}

/// Signal numbers, which differ between Unix systems; no `libc` is in the
/// graph (FG1), so they are stated here per target. A target not listed
/// classifies no signal: every one is a protocol failure, never a pass.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) mod signal {
    /// `(SIGSEGV, SIGBUS, SIGABRT, SIGFPE, SIGKILL, SIGXCPU, SIGXFSZ)`.
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    pub const TABLE: Option<[i32; 7]> = Some([11, 10, 6, 8, 9, 24, 25]);
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        any(target_arch = "mips", target_arch = "mips64")
    ))]
    pub const TABLE: Option<[i32; 7]> = Some([11, 10, 6, 8, 9, 30, 31]);
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        any(target_arch = "sparc", target_arch = "sparc64")
    ))]
    pub const TABLE: Option<[i32; 7]> = Some([11, 10, 6, 8, 9, 24, 25]);
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        not(any(
            target_arch = "mips",
            target_arch = "mips64",
            target_arch = "sparc",
            target_arch = "sparc64"
        ))
    ))]
    pub const TABLE: Option<[i32; 7]> = Some([11, 7, 6, 8, 9, 24, 25]);
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        target_os = "linux",
        target_os = "android"
    )))]
    pub const TABLE: Option<[i32; 7]> = None;

    /// The names, in [`TABLE`]'s order.
    pub const NAMES: [&str; 7] = [
        "SIGSEGV", "SIGBUS", "SIGABRT", "SIGFPE", "SIGKILL", "SIGXCPU", "SIGXFSZ",
    ];

    /// The number of the signal named `name` on this target.
    #[cfg(test)]
    pub fn number(name: &str) -> Option<i32> {
        let i = NAMES.iter().position(|n| *n == name)?;
        TABLE.map(|t| t[i])
    }
}

/// FV8 on Unix: a helper fitment did not kill ended by signal `number`.
pub(crate) fn by_signal(number: i32) -> ExitClass {
    let Some(table) = signal::TABLE else {
        return ExitClass::Other;
    };
    match table.iter().position(|&s| s == number) {
        Some(0..=3) => ExitClass::Crash,
        Some(4..=6) => ExitClass::Resource,
        _ => ExitClass::Other,
    }
}

/// FV8 on Windows: a helper fitment did not kill exited with `code`; an
/// NTSTATUS error (`0xC0000000` and up) is a crash. Pure, so every OS's
/// tests check it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn by_windows_code(code: u32) -> ExitClass {
    if code & 0xC000_0000 == 0xC000_0000 {
        ExitClass::Crash
    } else {
        ExitClass::Other
    }
}

/// FV8: how a helper fitment did not kill ended.
pub(crate) fn classify(status: ExitStatus) -> ExitClass {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map_or(ExitClass::Other, by_signal)
    }
    #[cfg(windows)]
    {
        // The code is a DWORD; `code()` gives its bits as an i32.
        #[allow(clippy::cast_sign_loss)]
        status
            .code()
            .map_or(ExitClass::Other, |c| by_windows_code(c as u32))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = status;
        ExitClass::Other
    }
}

/// How a helper ended, for the log: its exit code or signal.
pub(crate) fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(number) = status.signal() {
            let name = signal::TABLE
                .and_then(|t| t.iter().position(|&s| s == number))
                .map_or("", |i| signal::NAMES[i]);
            return format!("signal {number} {name}").trim_end().to_owned();
        }
    }
    match status.code() {
        #[cfg(windows)]
        #[allow(clippy::cast_sign_loss)]
        Some(code) => format!("exit code {:#010X}", code as u32),
        #[cfg(not(windows))]
        Some(code) => format!("exit code {code}"),
        None => "an unknown end".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crashes_and_resource_kills_are_named_by_their_signals() {
        let Some(_) = signal::TABLE else {
            return;
        };
        for name in ["SIGSEGV", "SIGBUS", "SIGABRT", "SIGFPE"] {
            assert_eq!(
                by_signal(signal::number(name).unwrap()),
                ExitClass::Crash,
                "{name}"
            );
        }
        for name in ["SIGKILL", "SIGXCPU", "SIGXFSZ"] {
            assert_eq!(
                by_signal(signal::number(name).unwrap()),
                ExitClass::Resource,
                "{name}"
            );
        }
        // SIGTERM, SIGINT, SIGHUP: none of FV8's.
        for other in [15, 2, 1, 0, -1, 64] {
            assert_eq!(by_signal(other), ExitClass::Other, "{other}");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_signal_numbers_are_the_systems() {
        // signal(7) on Linux's common architectures, signal(3) on macOS.
        let bus = if cfg!(target_os = "linux") { 7 } else { 10 };
        assert_eq!(signal::TABLE, Some([11, bus, 6, 8, 9, 24, 25]));
    }

    #[test]
    fn windows_crashes_are_ntstatus_errors() {
        for crash in [
            0xC000_0005,
            0xC000_00FD,
            0xC000_0409,
            0xC000_001D,
            0xC000_0094,
        ] {
            assert_eq!(by_windows_code(crash), ExitClass::Crash, "{crash:#X}");
        }
        for other in [0, 1, 3, 101, 0x8000_0003, 0x4000_0015] {
            assert_eq!(by_windows_code(other), ExitClass::Other, "{other:#X}");
        }
    }
}
