//! The start-up check of a front end (design m2 D.7 "回復を行う場面" 1, m3 B.12): what every
//! journal entry needs, judged unelevated (`mklm_core::attention`), summarized once so that the GUI
//! (banners, the recovery prompt, the post-reboot check) and the CLI (`recover`, the lost-helper
//! rule) read the same answer.

use mklm_core::{Attention, BootId, Journal, Liveness, OpState, ProcessIdentity, attention};

use crate::gate::{OpRef, post_reboot_entries};

/// One entry that needs something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionItem {
    pub op: OpRef,
    pub attention: Attention,
}

/// Every entry that needs something, in journal order (oldest first).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupSummary {
    /// Entries this build cannot read (design m2 C.10): writes stop until MKLM is updated.
    pub unreadable: usize,
    /// Entries whose attention is not `None`.
    pub items: Vec<AttentionItem>,
    /// Entries the post-reboot check asks about (`gate::post_reboot_entries`).
    pub post_reboot: Vec<OpRef>,
}

impl StartupSummary {
    /// Some entry needs the helper's `Recover` (interrupted, a countdown whose owner is gone, or
    /// a `PendingReboot` seen from a new boot).
    pub fn needs_recovery(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.attention == Attention::Recover)
    }

    /// The entries with `attention`.
    pub fn with(&self, attention: Attention) -> impl Iterator<Item = &AttentionItem> {
        self.items
            .iter()
            .filter(move |item| item.attention == attention)
    }

    /// Some entry stops new operations (design m2 C.7 `blocks_writes`), or the journal cannot be
    /// read.
    pub fn blocks_writes(&self) -> bool {
        self.unreadable > 0 || self.items.iter().any(|item| item.attention.blocks_writes())
    }

    /// The post-reboot check is due: an entry waits for it and the boot changed (a
    /// `PendingReboot` of this boot only needs the restart).
    pub fn post_reboot_due(&self) -> bool {
        self.post_reboot.iter().any(|op| {
            op.state == OpState::AwaitingConfirm
                || self
                    .items
                    .iter()
                    .any(|item| item.op.op_id == op.op_id && item.attention == Attention::Recover)
        })
    }
}

/// Summarizes `journal` for the boot `boot`; `liveness` tells whether an entry's owner still runs.
pub fn summarize(
    journal: &Journal,
    boot: BootId,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
) -> StartupSummary {
    StartupSummary {
        unreadable: journal.unreadable.len(),
        items: journal
            .entries
            .iter()
            .filter_map(|entry| {
                let attention = attention(entry, boot, liveness(&entry.owner));
                (attention != Attention::None).then(|| AttentionItem {
                    op: OpRef::of(entry),
                    attention,
                })
            })
            .collect(),
        post_reboot: post_reboot_entries(journal)
            .into_iter()
            .map(OpRef::of)
            .collect(),
    }
}
