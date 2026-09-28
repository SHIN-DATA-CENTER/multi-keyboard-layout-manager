//! H2: `mklm-update-runner.exe --run-update <run-id>` (design m5b D.7): step 1 here (COM
//! security, shutdown order, working folder, elevation and OS checks), then
//! `mklm_update::run_flow::run_update` over a `RunnerEnv` implemented with mklm-win.
//!
//! WP-H implements it. The skeleton does nothing and exits 7 ("not installed", no record).

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use mklm_ipc::RunUpdateArgs;

use crate::exit;

/// Runs the update and returns the process exit code (design m5b D.15).
pub fn run(args: &RunUpdateArgs) -> u8 {
    // The development branch of `for_this_build` references the dev marker, so that every
    // development build of the helper carries it (design m5b A.10, positive control).
    let _anchors = mklm_update::TrustAnchors::for_this_build();
    exit::NOT_UPDATED
}
