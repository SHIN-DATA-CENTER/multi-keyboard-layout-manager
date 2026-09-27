//! The restart screen (plan 3.6; design m3 B.8): why a restart is needed, what the user must know
//! before it (save files; a password with symbols may need other keys; sign in with a PIN), the
//! acknowledgement, and "restart now" only once the helper has flushed `PendingReboot`.

use mklm_core::{BootId, Journal};

use crate::i18n::Lang;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Restart {
    /// One line per entry of `mklm_client::gate::restart_reasons`.
    pub reasons: Vec<String>,
    /// Some entry is `PendingReboot` / `RevertedPendingReboot` in the journal (flushed, plan
    /// 2.3), or carries an `apply_pending` restart of this boot.
    pub ready: bool,
    /// The layout of some keyboard changes at the restart: show the password warning ("同じ
    /// キーで別の記号が入力されます。PIN（数字）でサインインするか…", review U15 d).
    pub layout_changes: bool,
}

/// Rules (WP-U4): from `mklm_client::gate::restart_reasons(journal, boot)`; each reason is the
/// operation in words (`vm::journal::kind_text`) and its state.
pub fn restart(journal: &Journal, boot: BootId, lang: Lang) -> Restart {
    let _ = (journal, boot, lang);
    todo!("WP-U4: the restart view-model")
}
