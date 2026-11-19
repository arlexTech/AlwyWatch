// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch core facade.
//!
//! Everything the native UIs need is exported from here. Android reaches it through
//! UniFFI-generated Kotlin bindings; the desktop app links it as a normal Rust crate.

uniffi::setup_scaffolding!();

/// Version of the core library, shown in the about screens.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Skeleton smoke test used by the hello-world screens.
#[uniffi::export]
pub fn greet(name: String) -> String {
    format!(
        "Hello {name}, from the AlwyWatch Rust core {}",
        core_version()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_mentions_name_and_version() {
        let s = greet("TV".into());
        assert!(s.contains("TV"));
        assert!(s.contains(env!("CARGO_PKG_VERSION")));
    }
}
