//! The UniFFI binding generator for this crate. `cargo run --bin uniffi-bindgen
//! -- generate --library <path-to-.so> --language kotlin --out-dir <dir>` emits
//! the Kotlin bindings the Android Gradle module consumes (driven by
//! [scripts/android-ffi-build.sh](../../../../scripts/android-ffi-build.sh)).

fn main() {
    uniffi::uniffi_bindgen_main();
}
