//! Tell cargo that the UI is an input.
//!
//! `RustEmbed` bakes `ui/` into the binary at compile time, but cargo has no
//! idea that directory exists — it watches `src/`. So editing the shell and
//! rebuilding can report `Finished` without re-embedding anything, and the
//! change silently does not ship. That is a particularly nasty failure because
//! everything reports success: the build passes, the daemon starts, and the
//! page is simply the old one.
//!
//! Listing the assets here makes them real dependencies.

use std::path::Path;

fn main() {
    // The manifest dir is crates/prismd; the assets are two levels up.
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    println!("cargo:rerun-if-changed=build.rs");
    watch(&ui);
}

fn watch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // A missing ui/ is a real problem, but it is the embed macro's problem
        // to report — failing here would just hide it behind a worse message.
        return;
    };
    println!("cargo:rerun-if-changed={}", dir.display());
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            watch(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
