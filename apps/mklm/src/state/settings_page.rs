//! The flows of the settings page (design m3 B.14; WP-U7), pure like the rest of `state`:
//!
//! - "確認の時間を長くする（60 秒）": `settings.change.countdown_seconds`, sent with every request
//!   from then on (`state::start_request`, `vm::settings::with_countdown`).
//! - "アンインストール時にキーボードの設定を元に戻す" (plan 3.13): machine-wide in HKLM. Read
//!   unelevated on the I/O worker whenever the page opens and after the helper saved it; "変更…"
//!   opens the confirmation (what the new value means, what the Windows prompt looks like), and
//!   only its button starts `Request::SetMachineSettings` — never a UAC prompt without a button
//!   press (design m3 0.2 principle 6). The confirmation explains the whole prompt when this user
//!   has not seen the explanation yet; confirming counts as having read it (review U14).
//! - "すべてのキーボードを MKLM 導入前に戻す…": the change page's restore preview for every
//!   keyboard (`RestoreScope::All`, design m3 B.4), prepared like any change; its "キャンセル"
//!   comes back here.

use mklm_client::gate::Gate;
use mklm_core::ApplyOptions;
use mklm_ipc::Request;

use super::{
    AppMsg, AppState, ChangeDraft, Effect, OverlayKind, Page, SessionPhase, start_request,
    stop_identifying, update,
};
use crate::i18n::Lang;
use crate::vm::keytest::KeyTest;

/// Something that happened on the settings page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsMsg {
    /// "確認の時間を長くする（60 秒）" was switched.
    LongCountdown(bool),
    /// "変更…" next to "アンインストール時にキーボードの設定を元に戻す".
    UninstallChange,
    /// The confirmation's "キャンセル".
    UninstallCancel,
    /// The confirmation's button: save the new value through the helper.
    UninstallConfirm,
    /// What the I/O worker read of the machine-wide setting (`Err`: English reason).
    MachineSettingsRead(Result<bool, String>),
    /// "すべてのキーボードを MKLM 導入前に戻す…".
    RestoreAll,
}

/// Page-local state of the settings page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsPageState {
    /// "restore on uninstall" as last read: `None` until read.
    pub restore_on_uninstall: Option<Result<bool, String>>,
    /// The confirmation is open, for this new value.
    pub confirming: Option<bool>,
}

/// The page is in front and can be used: no helper session, no dialog.
fn page_free(state: &AppState) -> bool {
    state.page == Page::Settings
        && state.session == SessionPhase::Idle
        && state.overlay == OverlayKind::None
}

pub fn handle(state: &mut AppState, msg: SettingsMsg) -> Vec<Effect> {
    match msg {
        SettingsMsg::LongCountdown(on) => {
            let seconds = if on {
                mklm_core::LONG_COUNTDOWN_SECONDS
            } else {
                mklm_core::DEFAULT_COUNTDOWN_SECONDS
            };
            if state.settings.change.countdown_seconds == seconds {
                return Vec::new();
            }
            state.settings.change.countdown_seconds = seconds;
            vec![
                Effect::SaveSettings(Box::new(state.settings.clone())),
                Effect::Render,
            ]
        }
        SettingsMsg::UninstallChange => {
            let Some(&Ok(current)) = state.settings_page.restore_on_uninstall.as_ref() else {
                return Vec::new();
            };
            if !page_free(state) || state.settings_page.confirming.is_some() {
                return Vec::new();
            }
            state.settings_page.confirming = Some(!current);
            vec![Effect::Render]
        }
        SettingsMsg::UninstallCancel => {
            if state.settings_page.confirming.take().is_none() {
                return Vec::new();
            }
            vec![Effect::Render]
        }
        SettingsMsg::UninstallConfirm => confirm(state),
        SettingsMsg::MachineSettingsRead(read) => {
            // A confirmation for the value that is set already has nothing left to do.
            if let (Some(on), Ok(current)) = (state.settings_page.confirming, &read)
                && on == *current
            {
                state.settings_page.confirming = None;
            }
            if read.is_err() {
                state.settings_page.confirming = None;
            }
            state.settings_page.restore_on_uninstall = Some(read);
            vec![Effect::Render]
        }
        SettingsMsg::RestoreAll => restore_all(state),
    }
}

/// The confirmation's button: `Request::SetMachineSettings` with the value it names, only while
/// that value is not the one read (the helper writes it under the write lock and flushes it).
fn confirm(state: &mut AppState) -> Vec<Effect> {
    let (Some(on), Some(Ok(current))) = (
        state.settings_page.confirming,
        state.settings_page.restore_on_uninstall.as_ref(),
    ) else {
        return Vec::new();
    };
    if !page_free(state) || on == *current {
        return Vec::new();
    }
    state.settings_page.confirming = None;
    let mut effects = Vec::new();
    if !state.elevated && !state.settings.change.uac_notice_seen {
        // The confirmation showed the whole explanation of the prompt (M2 R12): from now on one
        // line is enough, as after the change page's standalone explanation (review U14).
        state.settings.change.uac_notice_seen = true;
        effects.push(Effect::SaveSettings(Box::new(state.settings.clone())));
    }
    effects.extend(start_request(
        state,
        Request::SetMachineSettings {
            restore_on_uninstall: on,
        },
        ApplyOptions::default(),
        Vec::new(),
    ));
    effects
}

/// "すべてのキーボードを MKLM 導入前に戻す…" (design m3 B.4, B.14): the change page's restore
/// preview of everything MKLM changed; nothing is sent before its button.
fn restore_all(state: &mut AppState) -> Vec<Effect> {
    if !page_free(state) {
        return Vec::new();
    }
    let lang = state.lang.unwrap_or(Lang::Ja);
    let token = state.next_prepare;
    state.next_prepare += 1;
    state.settings_page.confirming = None;
    state.draft = Some(ChangeDraft {
        restore: true,
        restore_all: true,
        preparing: Some(token),
        ..ChangeDraft::new(String::new(), crate::i18n::every_keyboard(lang), Vec::new())
    });
    stop_identifying(state);
    state.key_test = KeyTest::default();
    state.page = Page::Change;
    vec![
        Effect::PrepareChange {
            token,
            gate: Gate::Restore,
        },
        Effect::Render,
    ]
}

/// A page was entered (by navigation or a flow): the confirmation belongs to the settings page
/// and closes when it goes; opening the page reads the machine-wide value again.
pub fn entered(state: &mut AppState, page: Page) -> Vec<Effect> {
    state.settings_page.confirming = None;
    if page == Page::Settings {
        vec![Effect::ReadMachineSettings]
    } else {
        Vec::new()
    }
}

/// "キャンセル" on the restore preview of everything: back to the settings page.
pub fn cancel_restore_all(state: &mut AppState) -> Option<Vec<Effect>> {
    if !state.draft.as_ref().is_some_and(|draft| draft.restore_all) {
        return None;
    }
    Some(update(state, AppMsg::Navigate(Page::Settings)))
}

/// After a session: once the helper saved the machine-wide value, read it again (the page shows
/// what is stored, not what was asked for).
pub fn session_ended(state: &AppState) -> Vec<Effect> {
    if matches!(state.request, Some(Request::SetMachineSettings { .. })) {
        vec![Effect::ReadMachineSettings]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::orchestrator::{RequestEnd, RequestReport};
    use mklm_client::run_once::RunOnceOutcome;
    use mklm_client::session::SessionEnd;
    use mklm_core::Outcome;

    use super::*;
    use crate::state::SessionOutcome;

    fn settings_page() -> AppState {
        let mut state = AppState::default();
        update(&mut state, AppMsg::Navigate(Page::Settings));
        state
    }

    fn read(state: &mut AppState, value: Result<bool, String>) -> Vec<Effect> {
        update(
            state,
            AppMsg::Settings(SettingsMsg::MachineSettingsRead(value)),
        )
    }

    fn settings(state: &mut AppState, msg: SettingsMsg) -> Vec<Effect> {
        update(state, AppMsg::Settings(msg))
    }

    #[test]
    fn the_longer_countdown_is_saved_and_sent_with_every_request() {
        let mut state = settings_page();
        let effects = settings(&mut state, SettingsMsg::LongCountdown(true));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveSettings(saved), Effect::Render] if saved.change.countdown_seconds == 60
        ));
        assert!(settings(&mut state, SettingsMsg::LongCountdown(true)).is_empty());
        // Every request from now on carries it (the helper counts down, design m3 B.6).
        let apply = crate::vm::change::ApplyMethod::Live.options();
        let effects = update(
            &mut state,
            AppMsg::StartRequest {
                request: Request::Undo { apply },
                apply,
            },
        );
        let Effect::StartSession {
            request: Request::Undo { apply: carried },
            apply,
            ..
        } = &effects[0]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(
            (carried.countdown_seconds, apply.countdown_seconds),
            (60, 60)
        );
        assert!(carried.allow_live_reset);
        let mut state = settings_page();
        state.settings.change.countdown_seconds = 60;
        let effects = settings(&mut state, SettingsMsg::LongCountdown(false));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveSettings(saved), Effect::Render] if saved.change.countdown_seconds == 20
        ));
    }

    #[test]
    fn opening_the_page_reads_the_machine_setting() {
        let mut state = AppState::default();
        let effects = update(&mut state, AppMsg::Navigate(Page::Settings));
        assert!(effects.contains(&Effect::ReadMachineSettings));
        assert!(
            !update(&mut state, AppMsg::Navigate(Page::About))
                .contains(&Effect::ReadMachineSettings)
        );
    }

    #[test]
    fn the_uninstall_setting_prompts_only_after_the_confirmation_button() {
        let mut state = settings_page();
        // Unknown: "変更…" does nothing.
        assert!(settings(&mut state, SettingsMsg::UninstallChange).is_empty());
        read(&mut state, Ok(true));
        assert_eq!(
            settings(&mut state, SettingsMsg::UninstallChange),
            vec![Effect::Render]
        );
        assert_eq!(state.settings_page.confirming, Some(false));
        // Opening the confirmation starts nothing; cancelling closes it.
        assert_eq!(state.session, SessionPhase::Idle);
        assert_eq!(
            settings(&mut state, SettingsMsg::UninstallCancel),
            vec![Effect::Render]
        );
        assert!(settings(&mut state, SettingsMsg::UninstallConfirm).is_empty());
        // The button: the whole explanation was read (first prompt), then the helper.
        settings(&mut state, SettingsMsg::UninstallChange);
        let effects = settings(&mut state, SettingsMsg::UninstallConfirm);
        assert!(
            matches!(effects.as_slice(), [Effect::SaveSettings(saved), Effect::StartSession {
                request: Request::SetMachineSettings { restore_on_uninstall: false },
                ..
            }, Effect::Render] if saved.change.uac_notice_seen),
            "{effects:?}"
        );
        assert!(state.settings_page.confirming.is_none());
        // A second press while the prompt is up starts nothing (one session at a time).
        settings(&mut state, SettingsMsg::UninstallChange);
        assert!(settings(&mut state, SettingsMsg::UninstallConfirm).is_empty());
        // After the session the value is read again, and the result says what is saved.
        let effects = update(
            &mut state,
            AppMsg::SessionEnded {
                session: 0,
                outcome: Box::new(SessionOutcome {
                    report: RequestReport {
                        // What the engine answers (`Engine::set_machine_settings`).
                        first: RequestEnd::Ended(SessionEnd::Finished(
                            mklm_core::OperationResult {
                                op_id: None,
                                outcome: Outcome::Confirmed,
                                failure: None,
                                pending_action: None,
                                conflicts: Vec::new(),
                                inv_ps2_violation: None,
                                recovered: Vec::new(),
                                warnings: Vec::new(),
                            },
                        )),
                        lost_needs_recovery: None,
                        recovery: None,
                        recovery_skipped: None,
                    },
                    run_once: Ok(RunOnceOutcome::NotNeeded),
                }),
            },
        );
        assert!(effects.contains(&Effect::ReadMachineSettings));
        let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(
            shown.message,
            "保存しました。MKLM をアンインストールしても、キーボードの設定は今のまま残ります。"
        );
    }

    #[test]
    fn an_unreadable_setting_or_another_page_closes_the_confirmation() {
        let mut state = settings_page();
        read(&mut state, Ok(false));
        settings(&mut state, SettingsMsg::UninstallChange);
        assert_eq!(state.settings_page.confirming, Some(true));
        // Read again with the new value already set elsewhere: nothing left to confirm.
        read(&mut state, Ok(true));
        assert!(state.settings_page.confirming.is_none());
        settings(&mut state, SettingsMsg::UninstallChange);
        read(&mut state, Err("denied".into()));
        assert!(state.settings_page.confirming.is_none());
        assert!(settings(&mut state, SettingsMsg::UninstallChange).is_empty());
        read(&mut state, Ok(true));
        settings(&mut state, SettingsMsg::UninstallChange);
        update(&mut state, AppMsg::Navigate(Page::About));
        assert!(state.settings_page.confirming.is_none());
        // An elevated GUI shows no prompt, so nothing is marked as explained.
        let mut state = settings_page();
        state.elevated = true;
        read(&mut state, Ok(true));
        settings(&mut state, SettingsMsg::UninstallChange);
        let effects = settings(&mut state, SettingsMsg::UninstallConfirm);
        assert!(
            matches!(effects[0], Effect::StartSession { .. }),
            "{effects:?}"
        );
    }

    #[test]
    fn restoring_everything_previews_first_and_cancels_back_to_settings() {
        let mut state = settings_page();
        let effects = settings(&mut state, SettingsMsg::RestoreAll);
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::PrepareChange {
                    gate: Gate::Restore,
                    ..
                },
                Effect::Render
            ]
        ));
        assert_eq!(state.page, Page::Change);
        let draft = state.draft.clone().unwrap();
        assert!(draft.restore && draft.restore_all && draft.members.is_empty());
        assert_eq!(state.session, SessionPhase::Idle);
        let effects = update(&mut state, AppMsg::CancelChange);
        assert_eq!(state.page, Page::Settings);
        assert!(state.draft.is_none());
        assert!(effects.contains(&Effect::ReadMachineSettings));
        // Only from the settings page itself.
        let mut state = AppState::default();
        assert!(settings(&mut state, SettingsMsg::RestoreAll).is_empty());
    }
}
