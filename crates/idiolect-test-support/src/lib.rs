//! Test support utilities for Idiolect contract tests.

pub mod fakes;
pub mod fixtures;
// Shells out to `/bin/sh` and sets Unix permission bits, so it only exists
// where that means something. `idiolect-application` depends on this crate and
// is built for Windows, which is where an unconditional `PermissionsExt`
// import breaks the build.
#[cfg(unix)]
pub mod notifications;

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
