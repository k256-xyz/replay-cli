//! Capture the git SHA at build time so `--version` can show
//! which commit the binary was built from. Codex's round-3 review:
//! "`--version` only prints 0.1.0; a user cannot verify they are on a
//! specific commit." Fix it by stamping the short SHA into a compile-
//! time env var that `main.rs` reads.
//!
//! Falls back to `unknown` when:
//!   - the build runs outside a git checkout (`cargo install --git`
//!     unpacks into a CARGO_REGISTRY dir without `.git`)
//!   - `git` is not on PATH on the build host
//!
//! Distributed binaries published via `cargo install --git ...` get
//! the SHA from the source archive's `.git` directory.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let stamp = if dirty {
        format!("{sha}-dirty")
    } else {
        sha
    };

    println!("cargo:rustc-env=K256_REPLAY_CLI_GIT_SHA={stamp}");
    // Re-stamp when HEAD or the index moves. (`cargo:rerun-if-changed`
    // on a file that doesn't exist is harmless when not in a checkout.)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
