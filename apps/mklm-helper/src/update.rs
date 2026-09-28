//! H1: a `StageUpdate` in a pipe session (design m5b D.4) and the `RecordTrust` handling (C.4):
//! `mklm_ipc::staging::{stage_update, record_trust}` over a `StagerEnv` implemented with mklm-win.
//!
//! WP-H implements it. The skeleton refuses without touching anything.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use mklm_ipc::{StageUpdateRequest, TrustReport, UpdateMessage, UpdateRefusal};

/// The answer to one `StageUpdate` (skeleton: `Refused(Internal)`; the session goes on).
pub fn stage(request: &StageUpdateRequest) -> UpdateMessage {
    // The development branch of `for_this_build` references the dev marker, so that every
    // development build of the helper carries it (design m5b A.10, positive control).
    let _anchors = mklm_update::TrustAnchors::for_this_build();
    UpdateMessage::Refused(skeleton())
}

/// The answer to a `RecordTrust` (skeleton: `TrustNotRecorded(Internal)`; the session goes on).
pub fn record_trust(report: &TrustReport) -> UpdateMessage {
    let _anchors = mklm_update::TrustAnchors::for_this_build();
    UpdateMessage::TrustNotRecorded(skeleton())
}

/// What the WP-0 skeleton answers (design m5b G.2).
fn skeleton() -> UpdateRefusal {
    UpdateRefusal::Internal {
        detail: "not implemented (m5b skeleton)".to_string(),
    }
}
