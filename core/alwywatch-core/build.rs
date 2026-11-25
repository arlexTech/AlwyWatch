// SPDX-License-Identifier: AGPL-3.0-or-later
//! `alwywatch_ffi`: files shared with the web app (`track_rules.rs`) export to UniFFI only here.
fn main() {
    println!("cargo::rustc-cfg=alwywatch_ffi");
}
