//! Crate documentation for the Idiolect workspace.

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

pub mod domain {
    pub mod adapter;
    pub mod candidate;
    pub mod events;
    pub mod session;
}

pub mod rules {
    pub mod session_lifecycle;
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
