// SPDX-License-Identifier: MIT
//! Links libsamplerate, which `src/samplerate.rs` binds.

fn main() {
    if let Err(error) = pkg_config::probe_library("samplerate") {
        panic!("libsamplerate through pkg-config: {error}");
    }
}
