// Runtime parity self-test, wired into `cargo test`: runs scripts/selftest.mjs
// against the freshly built `odw` binary so the gate covers the JS runtime
// behaviors (concurrency, determinism guard, worktree, budget, nested
// workflow, per-phase model) that Rust unit tests cannot reach.
//
// Requires `node` on PATH. odw verifies its own runtime here.

use std::process::Command;

#[test]
fn parity_selftest_passes() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let odw = env!("CARGO_BIN_EXE_odw");

    let output = Command::new("node")
        .arg("scripts/selftest.mjs")
        .current_dir(manifest)
        .env("ODW", odw)
        .output()
        .expect("failed to launch `node scripts/selftest.mjs` (is node installed?)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "parity selftest failed (exit {:?}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
}
