//! The taskbar-corner icon (plan 3.9; design m3 B.16): Slint's `SystemTrayIcon` as proven by the
//! prototype (M0 #8 E, F), re-created when explorer restarts (`TaskbarCreated` from the shell
//! window, because Slint 1.18.1 does not re-add it).
//!
//! The one-time close notice is not a balloon: Windows 11 swallows balloons under Do Not Disturb
//! or with notifications off, and hides new icons behind "^", so the first × always shows the
//! in-window notice (`Overlay::CloseNotice`, `state::update` on `WindowCloseRequested`) and hides
//! the window only on its OK (design m3 B.16, review U13).
//!
//! The tooltip names the most important journal attention ("MKLM — PC の再起動を待っている変更が
//! あります"), and "確認待ちの変更をすべて元に戻す…" is enabled only while such changes exist and
//! no change or session is under way ([`TrayState`]); both are refreshed on every render, also
//! while the window is hidden. The menu items that need the window show it first.

use mklm_client::startup::StartupSummary;
use mklm_core::Attention;
use slint::{Image, PlatformError};

use crate::app::dispatch;
use crate::i18n::{self, Lang};
use crate::state::AppMsg;
use crate::ui::TrayIcon;

/// Creates the icon with its menu wired to the app.
pub fn create_tray(icon: &Image) -> Result<TrayIcon, PlatformError> {
    let tray = TrayIcon::new()?;
    tray.set_tray_image(icon.clone());
    tray.on_icon_clicked(|| dispatch(AppMsg::Activate));
    tray.on_show_window(|| dispatch(AppMsg::Activate));
    // Both need the window: it is shown first (it may be hidden in the taskbar corner). Raw
    // Input reaches only the focused window; `StartIdentify` shows it itself (design m3 B.3).
    tray.on_identify(|| dispatch(AppMsg::StartIdentify));
    // The undo preview (design m3 B.12) on the recovery page; over a flow under way the window
    // only comes to the front.
    tray.on_undo_open(|| {
        dispatch(AppMsg::Activate);
        dispatch(AppMsg::Journal(crate::state::JournalMsg::OpenUndo));
    });
    // "MKLM を更新…" (design m5b E.3): the update page, in front.
    tray.on_update_open(|| {
        dispatch(AppMsg::Activate);
        dispatch(AppMsg::Update(crate::state::UpdateMsg::OpenPage));
    });
    tray.on_quit_app(|| dispatch(AppMsg::QuitRequested));
    tray.show()?;
    Ok(tray)
}

/// What the icon shows besides its image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayState {
    pub tooltip: String,
    /// Changes wait for the user's keep or revert (`AwaitingConfirm`, `PendingReboot`,
    /// `Conflict`: what `Request::Undo` puts back, design m3 B.12).
    pub can_undo: bool,
    /// "MKLM を更新…": an update is ready and nothing is under way (design m5b E.3).
    pub can_update: bool,
}

/// `state` with an update's part (design m5b E.3): its tooltip when the journal names nothing
/// (the journal's attention comes first), and the update item.
pub fn with_update(state: TrayState, tooltip: Option<String>, can_update: bool) -> TrayState {
    TrayState {
        tooltip: match tooltip {
            Some(tooltip) if state.tooltip == "MKLM" => tooltip,
            _ => state.tooltip,
        },
        can_update,
        ..state
    }
}

/// The icon's state for the last journal summary (`None`: not read yet). `can_navigate`: no
/// change, check or session is under way (`state::navigation_enabled`), so the undo item may
/// open its page.
pub fn tray_state(summary: Option<&StartupSummary>, can_navigate: bool, lang: Lang) -> TrayState {
    let Some(summary) = summary else {
        return TrayState {
            tooltip: i18n::tray_tooltip(None, false, false, lang),
            can_undo: false,
            can_update: false,
        };
    };
    let top = crate::vm::status::PRIORITY
        .into_iter()
        .find(|attention| summary.with(*attention).next().is_some());
    TrayState {
        tooltip: i18n::tray_tooltip(top, summary.post_reboot_due(), summary.unreadable > 0, lang),
        can_undo: can_navigate
            && [
                Attention::AwaitingUser,
                Attention::WaitingForReboot,
                Attention::Conflict,
            ]
            .into_iter()
            .any(|attention| summary.with(attention).next().is_some()),
        can_update: false,
    }
}

/// Shows `state` on `tray`.
pub fn update_tray(tray: &TrayIcon, state: &TrayState) {
    tray.set_tray_tooltip(state.tooltip.as_str().into());
    tray.set_can_undo(state.can_undo);
    tray.set_can_update(state.can_update);
}

#[cfg(test)]
mod tests {
    use mklm_client::gate::OpRef;
    use mklm_client::startup::AttentionItem;
    use mklm_core::{Layout, OpId, OpKind, OpState};

    use super::*;

    fn summary(attentions: &[Attention]) -> StartupSummary {
        StartupSummary {
            items: attentions
                .iter()
                .map(|attention| AttentionItem {
                    op: OpRef {
                        op_id: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
                        kind: OpKind::Migrate {
                            standard: Layout::Jis,
                            assignments: Vec::new(),
                        },
                        state: OpState::PendingReboot,
                    },
                    attention: *attention,
                })
                .collect(),
            ..StartupSummary::default()
        }
    }

    #[test]
    fn the_tooltip_and_the_undo_item_follow_the_journal() {
        assert_eq!(
            tray_state(None, true, Lang::Ja),
            TrayState {
                tooltip: "MKLM".into(),
                can_undo: false,
                can_update: false,
            }
        );
        assert_eq!(
            tray_state(Some(&StartupSummary::default()), true, Lang::En).tooltip,
            "MKLM"
        );
        let waiting = tray_state(
            Some(&summary(&[Attention::WaitingForReboot])),
            true,
            Lang::Ja,
        );
        assert_eq!(
            waiting.tooltip,
            "MKLM — PC の再起動を待っている変更があります"
        );
        assert!(waiting.can_undo);
        // Not while a change or a session runs (the navigation is off then, design m3 B.0).
        let busy = tray_state(
            Some(&summary(&[Attention::WaitingForReboot])),
            false,
            Lang::Ja,
        );
        assert_eq!(busy.tooltip, waiting.tooltip);
        assert!(!busy.can_undo);
        // The banner's order: a recovery comes before a conflict.
        let both = tray_state(
            Some(&summary(&[Attention::Conflict, Attention::Recover])),
            true,
            Lang::En,
        );
        assert_eq!(
            both.tooltip,
            format!(
                "MKLM — {}",
                i18n::attention(Attention::Recover, Lang::En).unwrap()
            )
        );
        assert!(both.can_undo);
        // Something to recover or apply is not undone by "undo every change waiting for you".
        let recover = tray_state(Some(&summary(&[Attention::Recover])), true, Lang::En);
        assert!(!recover.can_undo);
        let unreadable = StartupSummary {
            unreadable: 1,
            ..StartupSummary::default()
        };
        assert!(
            tray_state(Some(&unreadable), true, Lang::En)
                .tooltip
                .contains("journal cannot be read")
        );
    }

    /// An update names itself when the journal names nothing (design m5b E.3).
    #[test]
    fn a_ready_update_in_the_tray() {
        let update = Some("MKLM — 新しい版（0.2.1）があります".to_string());
        let quiet = with_update(tray_state(None, true, Lang::Ja), update.clone(), true);
        assert_eq!(quiet.tooltip, "MKLM — 新しい版（0.2.1）があります");
        assert!(quiet.can_update);
        let waiting = with_update(
            tray_state(
                Some(&summary(&[Attention::WaitingForReboot])),
                true,
                Lang::Ja,
            ),
            update,
            false,
        );
        assert_eq!(
            waiting.tooltip,
            "MKLM — PC の再起動を待っている変更があります"
        );
        assert!(!waiting.can_update && waiting.can_undo);
    }
}
