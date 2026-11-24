// SPDX-License-Identifier: MIT
//! Links libsamplerate, which `src/samplerate.rs` binds, and libsystemd, whose sd-bus
//! `src/realtime.rs` calls.

fn main() {
    if let Err(error) = pkg_config::probe_library("samplerate") {
        panic!("libsamplerate through pkg-config: {error}");
    }
    if let Err(error) = pkg_config::probe_library("libsystemd") {
        panic!("libsystemd through pkg-config: {error}");
    }
}
