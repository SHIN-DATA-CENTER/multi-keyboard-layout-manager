//! The flows of the journal pages (design m3 B.8 to B.12, B.18) with fake reads: no window, no
//! helper, no registry.

use std::time::Instant;

use mklm_client::orchestrator::RequestEnd;
use mklm_client::session::SessionEnd;
use mklm_client::startup::summarize;
use mklm_core::{
    ApplyOptions, BootId, ErrorCode, ErrorInfo, InvPs2Violation, Journal, JournalEntry,
    KeyboardType, Liveness, OpState, PendingAction, PlanError, ProcessIdentity, RegValue,
    SystemSnapshot, fixtures,
};
use mklm_ipc::Request;

use super::*;
use crate::state::{AppMsg, AppState, Effect, OverlayKind, Page, SessionOutcome, SystemRead};
use crate::vm::keytest::SCANCODE_DIGIT2;
use crate::vm::test_journal::{BOOT, BUILT_IN, KEYCHRON, LATER_BOOT, entry, went_through};

const JAPANESE: u32 = 0x0411_0411;

/// A user who has read the standalone explanation of the Windows prompt before: the pages' own
/// flows (the first prompt is `the_first_prompt_of_a_user_is_explained_first`).
fn app() -> AppState {
    let mut state = AppState::default();
    state.settings.change.uac_notice_seen = true;
    state
}

/// A countdown whose owner is gone, on the development machine with the Keychron at JIS: the
/// recovery page opens by itself.
fn recovery_page_with_a_lost_countdown(state: &mut AppState) {
    let mut counting = entry(
        "14141414-0000-4000-8000-000000000023",
        33,
        OpState::AwaitingConfirm,
    );
    went_through(&mut counting, OpState::Restarting);
    went_through(&mut counting, OpState::AwaitingConfirm);
    counting.countdown = Some(mklm_core::Countdown {
        seconds: 20,
        deadline: mklm_core::Timestamp(0),
    });
    update(state, read(journal(vec![counting]), BOOT, keychron_jis()));
    assert_eq!(state.page, Page::Recovery);
}

/// Settings are per user, the journal per PC: another user's first prompt may come from the
/// recovery page. It is explained on its own page first, as before a change (design m3 B.5,
/// B.12; M2 R12); "キャンセル" goes back without a prompt, "確認画面へ進む" sends the request.
#[test]
fn the_first_prompt_of_a_user_is_explained_first() {
    let mut state = AppState::default();
    recovery_page_with_a_lost_countdown(&mut state);
    let effects = journal_msg(&mut state, JournalMsg::RecoveryPrimary);
    assert_eq!(effects, vec![Effect::Render]);
    assert_eq!(state.page, Page::UacNotice);
    assert_eq!(state.session, crate::state::SessionPhase::Idle);
    // Cancel: back to the recovery page, nothing sent, the explanation still unread.
    assert_eq!(
        update(&mut state, AppMsg::CancelChange),
        vec![Effect::Render]
    );
    assert_eq!(state.page, Page::Recovery);
    assert!(!state.settings.change.uac_notice_seen);
    assert!(state.journal_pages.waiting.is_none());
    // Again, and on to the prompt: the request of the page starts from its page.
    journal_msg(&mut state, JournalMsg::RecoveryPrimary);
    assert_eq!(state.page, Page::UacNotice);
    let effects = update(&mut state, AppMsg::UacGo);
    assert!(
        matches!(&effects[0], Effect::SaveSettings(saved) if saved.change.uac_notice_seen),
        "{effects:?}"
    );
    assert!(matches!(
        started(&effects),
        Some((Request::Recover { .. }, _))
    ));
    assert_eq!(state.page, Page::Recovery);
    // From now on, one line next to the button is enough.
    let mut state = app();
    recovery_page_with_a_lost_countdown(&mut state);
    let effects = journal_msg(&mut state, JournalMsg::RecoveryPrimary);
    assert!(started(&effects).is_some());
    // An elevated GUI shows no prompt, so none is explained.
    let mut elevated = AppState {
        elevated: true,
        ..AppState::default()
    };
    recovery_page_with_a_lost_countdown(&mut elevated);
    let effects = journal_msg(&mut elevated, JournalMsg::RecoveryPrimary);
    assert!(started(&effects).is_some());
}

fn read(journal: Journal, boot: BootId, snapshot: SystemSnapshot) -> AppMsg {
    let summary = summarize(&journal, boot, &|_: &ProcessIdentity| Liveness::Dead);
    AppMsg::SystemRead(Box::new(SystemRead {
        snapshot: Some(snapshot),
        warnings: Vec::new(),
        journal: Some(journal),
        boot: Some(boot),
        summary,
    }))
}

fn journal(entries: Vec<JournalEntry>) -> Journal {
    Journal {
        entries,
        ..Journal::default()
    }
}

/// The development machine after the Keychron was set to JIS through the restart.
fn keychron_jis() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    for kb in &mut snapshot.keyboards {
        if kb.instance_id == KEYCHRON {
            kb.overrides.keyboard_type_override = Some(7);
            kb.overrides.keyboard_subtype_override = Some(2);
            kb.reported_type = Some(KeyboardType::JIS);
        }
    }
    snapshot
}

fn restart_change(state: OpState) -> JournalEntry {
    let mut change = entry("12121212-0000-4000-8000-000000000021", 31, state);
    change.apply = Some(PendingAction::RestartPc);
    change
}

fn started(effects: &[Effect]) -> Option<(&Request, &ApplyOptions)> {
    effects.iter().find_map(|effect| match effect {
        Effect::StartSession { request, apply, .. } => Some((request, apply)),
        _ => None,
    })
}

fn journal_msg(state: &mut AppState, msg: JournalMsg) -> Vec<Effect> {
    update(state, AppMsg::Journal(msg))
}

fn key(state: &mut AppState, instance_id: &str, text: &str) -> Vec<Effect> {
    update(
        state,
        AppMsg::DeviceKey {
            instance_id: instance_id.into(),
            scancode: SCANCODE_DIGIT2,
            at: Instant::now(),
        },
    );
    update(
        state,
        AppMsg::KeyTestPressed {
            text: text.into(),
            shift: true,
            at: Instant::now(),
        },
    )
}

/// Design m3 B.9 (INTERACTIONS-4): the same GUI, moved to a Remote Desktop session after a
/// Shift+2 on the Keychron at the console. Keys typed through Remote Desktop come without a Raw
/// Input press of this PC's keyboards (docs/research/rdp-keyboard.md 6.3): they fill no row and get
/// no verdict — the Keychron's row keeps what the Keychron typed — and the key test asks for the
/// test at the PC. "このままにする" stays possible (the user decides, review U2).
#[test]
fn a_remote_desktop_key_fills_no_row_of_the_post_reboot_check() {
    let mut state = AppState {
        active_hkl: JAPANESE,
        ..app()
    };
    let pending = journal(vec![restart_change(OpState::PendingReboot)]);
    update(
        &mut state,
        read(pending.clone(), LATER_BOOT, keychron_jis()),
    );
    assert_eq!(state.page, Page::PostReboot);
    key(&mut state, KEYCHRON, "\"");
    assert_eq!(post_reboot_view(&state).rows[0].typed, "✓");
    assert!(state.last_key.is_none(), "the press is used once");
    // The Remote Desktop keyboard arrived: the next read says the session is remote.
    let mut remote = keychron_jis();
    remote.os.remote_session = true;
    update(&mut state, read(pending, LATER_BOOT, remote));
    assert_eq!(state.page, Page::PostReboot);
    assert!(crate::state::remote_session(&state));
    // Shift+2 from the client, whose session types US: no press of this PC came with it.
    update(
        &mut state,
        AppMsg::KeyTestPressed {
            text: "@".into(),
            shift: true,
            at: Instant::now(),
        },
    );
    let view = post_reboot_view(&state);
    assert_eq!(view.rows[0].typed, "✓");
    assert!(view.can_keep);
    assert_eq!(
        (
            state.key_test.last_text.as_str(),
            state.key_test.verdict.as_str(),
            state.key_test.device.as_str()
        ),
        ("@", "", "")
    );
    assert!(
        state
            .key_test
            .prompt
            .starts_with("リモート デスクトップで接続しています。"),
        "{}",
        state.key_test.prompt
    );
}

#[test]
fn the_post_reboot_check_opens_in_front_once_and_waits_for_the_user() {
    let mut state = AppState {
        active_hkl: JAPANESE,
        ..app()
    };
    let pending = journal(vec![restart_change(OpState::PendingReboot)]);
    let effects = update(
        &mut state,
        read(pending.clone(), LATER_BOOT, keychron_jis()),
    );
    assert_eq!(state.page, Page::PostReboot);
    assert!(state.visible);
    assert!(effects.contains(&Effect::ShowWindow));
    assert!(effects.contains(&Effect::Journal(JournalEffect::AlwaysOnTop(true))));
    // The navigation is off while the check waits (design m3 B.0).
    assert!(!crate::state::navigation_enabled(&state));
    let view = post_reboot_view(&state);
    assert!(view.can_keep && !view.not_restarted);
    assert_eq!(view.rows[0].typed, "未");
    // The key test on the Keychron fills its row; the first key ends the on-top period.
    let effects = key(&mut state, KEYCHRON, "\"");
    assert!(effects.contains(&Effect::Journal(JournalEffect::AlwaysOnTop(false))));
    assert_eq!(post_reboot_view(&state).rows[0].typed, "✓");
    assert_eq!(
        state.key_test.verdict,
        "Shift+2 → \" : ✓ 期待どおり JIS です"
    );
    // A key from the built-in keyboard is not the Keychron's row: no verdict against it.
    key(&mut state, BUILT_IN, "\"");
    assert_eq!(post_reboot_view(&state).rows.len(), 1);
    // Later: nothing changes, the RunOnce value is registered again, back to the main page.
    let effects = journal_msg(&mut state, JournalMsg::PostRebootLater);
    assert!(effects.contains(&Effect::RunOnceRule));
    assert_eq!(state.page, Page::Main);
    // It does not open by itself again in this run.
    update(&mut state, read(pending, LATER_BOOT, keychron_jis()));
    assert_eq!(state.page, Page::Main);
}

#[test]
fn keep_and_revert_on_the_post_reboot_check() {
    let mut state = app();
    let change = restart_change(OpState::PendingReboot);
    let op_id = change.op_id.clone();
    update(
        &mut state,
        read(journal(vec![change]), LATER_BOOT, keychron_jis()),
    );
    let effects = journal_msg(&mut state, JournalMsg::PostRebootKeep);
    assert_eq!(
        started(&effects),
        Some((
            &Request::Confirm {
                op_id: op_id.clone()
            },
            &ApplyOptions::default()
        ))
    );
    // One session at a time: revert waits for the first to end.
    assert!(started(&journal_msg(&mut state, JournalMsg::PostRebootRevert)).is_none());
    state.session = SessionPhase::Idle;
    let effects = journal_msg(&mut state, JournalMsg::PostRebootRevert);
    assert_eq!(
        started(&effects).map(|(request, _)| request),
        Some(&Request::Revert {
            op_id,
            apply: ApplyOptions::default()
        })
    );
}

#[test]
fn started_for_the_check_after_a_shutdown() {
    // `--post-reboot`, but the boot did not change (Fast Startup): the page says so, keep is
    // off, and the check is registered again (design m2 D.7 step 2, T-POST-3).
    let mut state = app();
    state.journal_pages.post_reboot.requested = true;
    let effects = update(
        &mut state,
        read(
            journal(vec![restart_change(OpState::PendingReboot)]),
            BOOT,
            keychron_jis(),
        ),
    );
    assert_eq!(state.page, Page::PostReboot);
    assert!(effects.contains(&Effect::RunOnceRule));
    let view = post_reboot_view(&state);
    assert!(view.not_restarted && !view.can_keep);
    assert!(started(&journal_msg(&mut state, JournalMsg::PostRebootKeep)).is_none());
    // Without `--post-reboot` the same journal only needs the restart: no page of its own.
    let mut plain = app();
    update(
        &mut plain,
        read(
            journal(vec![restart_change(OpState::PendingReboot)]),
            BOOT,
            keychron_jis(),
        ),
    );
    assert_eq!(plain.page, Page::Main);
}

#[test]
fn the_recovery_page_opens_once_per_entry_and_boot() {
    let mut state = app();
    let written = entry("13131313-0000-4000-8000-000000000022", 32, OpState::Written);
    let interrupted = journal(vec![written]);
    // Every value at `intended` (R5: the writer stopped before the reset).
    let effects = update(&mut state, read(interrupted.clone(), BOOT, keychron_jis()));
    assert_eq!(state.page, Page::Recovery);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::SaveSettings(_)))
    );
    assert_eq!(state.settings.recovery.prompted.len(), 1);
    assert_eq!(state.settings.recovery.prompted[0].boot, BOOT.to_text());
    // Never a UAC prompt by itself (design m3 K.6): the page waits for the button.
    assert!(started(&effects).is_none());
    // "後で": back to the main page; the next read does not open it again.
    journal_msg(&mut state, JournalMsg::RecoveryDismiss);
    assert_eq!(state.page, Page::Main);
    update(&mut state, read(interrupted.clone(), BOOT, keychron_jis()));
    assert_eq!(state.page, Page::Main);
    // The recovery itself: `Request::Recover`, no reset in place for a change that never reached
    // its reset (R5; the method is not offered).
    update(&mut state, AppMsg::Navigate(Page::Recovery));
    let view = recovery_view(&state, &|_| String::new());
    assert!(view.can_recover && !view.method.visible);
    // Even with "switch back now" chosen, nothing is reset where the page did not offer it.
    journal_msg(&mut state, JournalMsg::RecoveryChooseMethod(0));
    let effects = journal_msg(&mut state, JournalMsg::RecoveryPrimary);
    assert_eq!(
        started(&effects),
        Some((
            &Request::Recover {
                apply: ApplyOptions::default()
            },
            &ApplyOptions::default()
        ))
    );
}

#[test]
fn recovering_a_switched_keyboard_offers_the_method() {
    // The mouse was used a moment ago (design m3 B.5): "switch back now" is preset when the page
    // opens by itself, and stays what the page shows.
    let now = Instant::now();
    let mut state = AppState {
        now: Some(now),
        ..app()
    };
    update(&mut state, AppMsg::PointerUsed(now));
    let mut counting = entry(
        "14141414-0000-4000-8000-000000000023",
        33,
        OpState::AwaitingConfirm,
    );
    went_through(&mut counting, OpState::Restarting);
    went_through(&mut counting, OpState::AwaitingConfirm);
    counting.countdown = Some(mklm_core::Countdown {
        seconds: 20,
        deadline: mklm_core::Timestamp(0),
    });
    update(
        &mut state,
        read(journal(vec![counting]), BOOT, keychron_jis()),
    );
    assert_eq!(state.page, Page::Recovery);
    let view = recovery_view(&state, &|_| String::new());
    assert!(view.method.visible);
    assert_eq!(view.method.selected, 0);
    // The user picks the other way, and back: each is shown and sent as chosen.
    journal_msg(&mut state, JournalMsg::RecoveryChooseMethod(1));
    assert_eq!(recovery_view(&state, &|_| String::new()).method.selected, 1);
    assert!(journal_msg(&mut state, JournalMsg::RecoveryChooseMethod(1)).is_empty());
    journal_msg(&mut state, JournalMsg::RecoveryChooseMethod(0));
    let effects = journal_msg(&mut state, JournalMsg::RecoveryPrimary);
    let live = crate::vm::change::ApplyMethod::Live.options();
    assert_eq!(
        started(&effects),
        Some((&Request::Recover { apply: live }, &live))
    );
    // Without recent input (or without a time) the safe way is preset: no reset in place.
    let mut quiet = AppState {
        page: Page::Journal,
        ..app()
    };
    update(&mut quiet, AppMsg::Navigate(Page::Recovery));
    assert_eq!(
        quiet.journal_pages.recovery.method,
        Some(crate::vm::change::ApplyMethod::Restart)
    );
}

#[test]
fn restart_now_needs_the_acknowledgement_and_a_reason() {
    let mut state = app();
    update(
        &mut state,
        read(
            journal(vec![restart_change(OpState::PendingReboot)]),
            BOOT,
            keychron_jis(),
        ),
    );
    update(&mut state, AppMsg::Navigate(Page::Restart));
    assert!(restart_view(&state).ready);
    assert!(
        journal_msg(
            &mut state,
            JournalMsg::RestartNow {
                acknowledged: false
            }
        )
        .is_empty()
    );
    let effects = journal_msg(&mut state, JournalMsg::RestartNow { acknowledged: true });
    assert_eq!(
        effects,
        vec![Effect::Journal(JournalEffect::RestartPc), Effect::Render]
    );
    // Pressed twice: once.
    assert!(journal_msg(&mut state, JournalMsg::RestartNow { acknowledged: true }).is_empty());
    journal_msg(
        &mut state,
        JournalMsg::RestartFailed("InitiateShutdownW failed with Win32 error 1115".into()),
    );
    assert!(!state.journal_pages.restart.restarting);
    assert!(state.journal_pages.restart.error.is_some());
    // No reason in the journal: never.
    let mut idle = app();
    update(
        &mut idle,
        read(Journal::default(), BOOT, fixtures::dev_machine()),
    );
    update(&mut idle, AppMsg::Navigate(Page::Restart));
    assert!(journal_msg(&mut idle, JournalMsg::RestartNow { acknowledged: true }).is_empty());
    // "変更を元に戻す…" previews the undo; "キャンセル" comes back to the restart page.
    journal_msg(&mut state, JournalMsg::OpenUndo);
    assert_eq!(state.page, Page::Recovery);
    assert_eq!(state.journal_pages.recovery.mode, RecoveryMode::Undo);
    journal_msg(&mut state, JournalMsg::RecoveryDismiss);
    assert_eq!(state.page, Page::Restart);
}

fn conflict_entry_8_2() -> JournalEntry {
    let mut conflict = entry(
        "15151515-0000-4000-8000-000000000024",
        34,
        OpState::Conflict,
    );
    conflict.records[0].conflict = Some(RegValue::Dword { value: 8 });
    conflict
}

fn snapshot_8_2() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    for kb in &mut snapshot.keyboards {
        if kb.instance_id == KEYCHRON {
            kb.overrides.keyboard_type_override = Some(8);
            kb.overrides.keyboard_subtype_override = Some(2);
        }
    }
    snapshot
}

#[test]
fn a_conflict_is_resolved_per_keyboard() {
    let mut state = app();
    let conflict = conflict_entry_8_2();
    let op_id = conflict.op_id.clone();
    update(
        &mut state,
        read(journal(vec![conflict]), BOOT, snapshot_8_2()),
    );
    // Not opened by itself (the banner leads there).
    assert_eq!(state.page, Page::Main);
    update(&mut state, AppMsg::Navigate(Page::Conflict));
    let view = conflict_view(&state);
    assert_eq!(view.keyboards[0].selected, 0);
    assert_eq!(
        view.keyboards[0].options[0].label,
        "変更前の US に戻す（おすすめ）"
    );
    // The user picks "MKLM が設定した JIS にする"; both values of the pair follow.
    journal_msg(
        &mut state,
        JournalMsg::ConflictChoice {
            keyboard: 0,
            option: 1,
        },
    );
    let effects = journal_msg(&mut state, JournalMsg::ConflictResolve);
    let Some((Request::ResolveConflict(request), _)) = started(&effects) else {
        panic!("{effects:?}");
    };
    assert_eq!(request.op_id, op_id);
    assert!(
        request
            .choices
            .iter()
            .all(|choice| choice.choice == mklm_core::ResolutionChoice::UseIntended)
    );
    assert_eq!(request.choices.len(), 2);
    assert_eq!(request.apply, ApplyOptions::default());
    // The helper refused it for INV-PS2: the page says which PS/2 keyboard and the ways out.
    update(
        &mut state,
        AppMsg::SessionEnded {
            session: 0,
            outcome: Box::new(SessionOutcome {
                report: mklm_client::orchestrator::RequestReport {
                    first: RequestEnd::Ended(SessionEnd::Failed(ErrorInfo {
                        code: ErrorCode::PlanRejected,
                        message: "INV-PS2".into(),
                        op_id: Some(op_id),
                        plan_error: Some(PlanError::InvPs2(InvPs2Violation {
                            keyboards: vec![BUILT_IN.into()],
                        })),
                    })),
                    lost_needs_recovery: None,
                    recovery: None,
                    recovery_skipped: None,
                },
                run_once: Ok(mklm_client::run_once::RunOnceOutcome::NotNeeded),
            }),
        },
    );
    assert_eq!(state.overlay, OverlayKind::Result);
    let error = conflict_view(&state).error;
    assert!(
        error.contains("日本語 PS/2 キーボード (106/109 キー Ctrl+英数)"),
        "{error}"
    );
    // A new choice clears it.
    journal_msg(
        &mut state,
        JournalMsg::ConflictChoice {
            keyboard: 0,
            option: 0,
        },
    );
    assert_eq!(conflict_view(&state).error, "");
}

#[test]
fn the_history_reverts_through_its_preview() {
    let mut state = app();
    let history = crate::vm::test_journal::history();
    update(&mut state, read(history, BOOT, fixtures::dev_machine()));
    update(&mut state, AppMsg::Navigate(Page::Journal));
    let rows = journal_view(&state, &|at| {
        crate::vm::journal::utc_time_text(at, Lang::Ja)
    })
    .rows;
    let kept = rows.iter().find(|row| row.can_revert).unwrap();
    assert_eq!(kept.op, "3f2a9c1e");
    journal_msg(
        &mut state,
        JournalMsg::JournalRevert {
            op_id: kept.op_id.clone(),
        },
    );
    assert_eq!(state.page, Page::Recovery);
    let view = recovery_view(&state, &|_| "22:30".into());
    assert!(view.can_recover && view.method.visible);
    // "抜き差しか PC の再起動で戻す" (preset without recent input): no reset in place.
    assert_eq!(view.method.selected, 1);
    let effects = journal_msg(&mut state, JournalMsg::RecoveryPrimary);
    assert_eq!(
        started(&effects),
        Some((
            &Request::Revert {
                op_id: mklm_core::OpId::parse(&kept.op_id).unwrap(),
                apply: ApplyOptions::default()
            },
            &ApplyOptions::default()
        ))
    );
    // While the session runs, nothing else starts from these pages.
    assert!(journal_msg(&mut state, JournalMsg::RecoveryPrimary).is_empty());
    // The session ended and its result was closed; "キャンセル" of a revert preview goes back to
    // the history.
    state.session = SessionPhase::Idle;
    state.overlay = OverlayKind::None;
    journal_msg(&mut state, JournalMsg::RecoveryDismiss);
    assert_eq!(state.page, Page::Journal);
    // An operation the helper would not revert (R5, reverted already): the preview says so and
    // nothing starts.
    journal_msg(
        &mut state,
        JournalMsg::JournalRevert {
            op_id: "8e9a9970-f7bf-46c7-b779-f914f17bd40d".into(),
        },
    );
    assert_eq!(state.page, Page::Recovery);
    let refused = recovery_view(&state, &|_| String::new());
    assert!(!refused.can_recover && !refused.empty_note.is_empty());
    assert!(journal_msg(&mut state, JournalMsg::RecoveryPrimary).is_empty());
}

#[test]
fn keeping_a_change_that_needs_the_restart_goes_through_the_check() {
    // An AwaitingConfirm that took effect at the restart (after `RebootObserved`): kept only
    // after the check (design m2 D.6, review C2); a reconnect change is kept at once.
    let mut state = app();
    let mut after_restart = restart_change(OpState::AwaitingConfirm);
    after_restart.boot_id = BOOT;
    let reconnect = {
        let mut change = entry(
            "16161616-0000-4000-8000-000000000025",
            35,
            OpState::AwaitingConfirm,
        );
        change.apply = Some(PendingAction::Reconnect);
        change
    };
    let restart_op = after_restart.op_id.to_string();
    let reconnect_op = reconnect.op_id.clone();
    let mut stay = read(
        journal(vec![after_restart, reconnect]),
        BOOT,
        keychron_jis(),
    );
    // Keep the pages from opening by themselves for this test.
    if let AppMsg::SystemRead(read) = &mut stay {
        read.summary.post_reboot.clear();
    }
    state.page = Page::Recovery;
    update(&mut state, stay);
    journal_msg(
        &mut state,
        JournalMsg::RecoveryKeep {
            op_id: restart_op.clone(),
        },
    );
    assert_eq!(state.page, Page::PostReboot);
    assert_eq!(
        state
            .journal_pages
            .post_reboot
            .op_id
            .as_ref()
            .map(ToString::to_string),
        Some(restart_op)
    );
    let effects = journal_msg(
        &mut state,
        JournalMsg::RecoveryKeep {
            op_id: reconnect_op.to_string(),
        },
    );
    assert_eq!(
        started(&effects).map(|(request, _)| request),
        Some(&Request::Confirm {
            op_id: reconnect_op
        })
    );
}

#[test]
fn a_hidden_start_shows_the_window_for_a_conflict() {
    let mut state = app();
    update(
        &mut state,
        read(journal(vec![conflict_entry_8_2()]), BOOT, snapshot_8_2()),
    );
    assert!(state.visible);
    // Only for the first read of the run.
    state.visible = false;
    update(
        &mut state,
        read(journal(vec![conflict_entry_8_2()]), BOOT, snapshot_8_2()),
    );
    assert!(!state.visible);
}

#[test]
fn the_run_once_rule_outcome_is_kept_for_the_pages() {
    let mut state = app();
    journal_msg(
        &mut state,
        JournalMsg::RunOnceDone(Ok(mklm_client::run_once::RunOnceOutcome::TellUser)),
    );
    assert_eq!(state.journal_pages.run_once_problem, Some(true));
    journal_msg(
        &mut state,
        JournalMsg::RunOnceDone(Err(mklm_client::run_once::RunOnceError::Register(
            "denied".into(),
        ))),
    );
    assert_eq!(state.journal_pages.run_once_problem, Some(false));
    journal_msg(
        &mut state,
        JournalMsg::RunOnceDone(Ok(mklm_client::run_once::RunOnceOutcome::NotNeeded)),
    );
    assert_eq!(state.journal_pages.run_once_problem, None);
}

/// One value a restore reported as changed outside MKLM (8/2 on the Keychron, 4/0 before MKLM).
fn restore_conflict(record: usize, name: &str, baseline: u32, current: u32) -> ConflictInfo {
    let target = mklm_core::WriteTarget::Device {
        instance_id: KEYCHRON.into(),
    };
    ConflictInfo {
        op_id: mklm_core::OpId::parse("17171717-0000-4000-8000-000000000026").unwrap(),
        record,
        key_path: mklm_core::ValueKey {
            target,
            name: name.into(),
        }
        .key_path(),
        name: name.into(),
        baseline: RegValue::Dword { value: baseline },
        before: RegValue::Dword { value: baseline },
        intended: RegValue::Dword {
            value: baseline + 3,
        },
        last_written: Some(RegValue::Dword {
            value: baseline + 3,
        }),
        current: RegValue::Dword { value: current },
        write_error: None,
    }
}

#[test]
fn a_stopped_restore_is_sent_again_as_chosen() {
    // "MKLM 導入前に戻す" of the Keychron found 8/2 written by something else: nothing was
    // written; the result leads to the conflict page, which sends the restore again (design m3
    // B.10, `ConflictPolicy::Report` then `Skip` / `Overwrite`).
    let mut state = AppState {
        lang: Some(Lang::Ja),
        ..app()
    };
    update(&mut state, read(Journal::default(), BOOT, snapshot_8_2()));
    let scope = RestoreScope::Device {
        instance_id: KEYCHRON.into(),
    };
    let request = Request::RestoreBaseline(RestoreBaselineRequest {
        scope: scope.clone(),
        on_conflict: ConflictPolicy::Report,
        apply: ApplyOptions::default(),
    });
    update(
        &mut state,
        AppMsg::StartRequest {
            request,
            apply: ApplyOptions::default(),
        },
    );
    let stopped = |state: &mut AppState, session| {
        update(
            state,
            AppMsg::SessionEnded {
                session,
                outcome: Box::new(SessionOutcome {
                    report: mklm_client::orchestrator::RequestReport {
                        first: RequestEnd::Ended(SessionEnd::Finished(
                            mklm_core::OperationResult {
                                op_id: None,
                                outcome: mklm_core::Outcome::Conflict,
                                failure: None,
                                pending_action: None,
                                conflicts: vec![
                                    restore_conflict(0, mklm_core::value_names::HID_TYPE, 4, 8),
                                    restore_conflict(1, mklm_core::value_names::HID_SUBTYPE, 0, 2),
                                ],
                                inv_ps2_violation: None,
                                recovered: Vec::new(),
                                warnings: Vec::new(),
                            },
                        )),
                        lost_needs_recovery: None,
                        recovery: None,
                        recovery_skipped: None,
                    },
                    run_once: Ok(mklm_client::run_once::RunOnceOutcome::NotNeeded),
                }),
            },
        )
    };
    stopped(&mut state, 0);
    assert_eq!(state.overlay, OverlayKind::Result);
    update(&mut state, AppMsg::ResultReadArrived);
    let result = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
    assert_eq!(result.next, Some(crate::vm::result::NextStep::Conflict));
    update(&mut state, AppMsg::ResultNextStep);
    assert_eq!(state.page, Page::Conflict);
    let view = conflict_view(&state);
    assert_eq!(
        crate::vm::SnapshotText::snapshot_text(&view),
        "operation: MKLM 導入前に戻す（Keychron Receiver）\n\
         note: 「MKLM 導入前に戻す」で戻す値のうち、MKLM 以外が変更した値がありました。まだ何も戻していません。その値をどうするかを選んでください。\n\
         keyboard: Keychron Receiver\n  今の値: 不明な種類（8/2） — MKLM 以外が変更\n  MKLM 導入前: US\n\
         \x20 · Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters\\KeyboardTypeOverride: \
         今の値 8 / MKLM が最後に書いた値 7 / 操作の前の値 4 / 操作で書こうとした値 7 / MKLM 導入前の値 4\n\
         \x20 · Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters\\KeyboardSubtypeOverride: \
         今の値 2 / MKLM が最後に書いた値 3 / 操作の前の値 0 / 操作で書こうとした値 3 / MKLM 導入前の値 0\n\
         ○ MKLM 以外が変えた値はそのままにする（ほかの値は導入前に戻します）\n\
         ◉ MKLM 以外が変えた値も MKLM 導入前の値に戻す（おすすめ）\n"
    );
    assert!(view.can_resolve());
    // The user keeps the values changed outside MKLM: the restore goes again with `Skip`.
    journal_msg(&mut state, JournalMsg::ConflictRestoreChoice(0));
    assert_eq!(conflict_view(&state).restore_selected, 0);
    let effects = journal_msg(&mut state, JournalMsg::ConflictResolve);
    assert_eq!(
        started(&effects).map(|(request, _)| request),
        Some(&Request::RestoreBaseline(RestoreBaselineRequest {
            scope,
            on_conflict: ConflictPolicy::Skip,
            apply: ApplyOptions::default(),
        }))
    );
    // Stopped again, but the result closed without its next step: the page no longer asks
    // about it (it shows the journal's conflicts, none here).
    stopped(&mut state, 1);
    update(&mut state, AppMsg::ResultClosed);
    assert!(state.journal_pages.conflict.restore.is_none());
    update(&mut state, AppMsg::Navigate(Page::Conflict));
    let view = conflict_view(&state);
    assert!(!view.can_resolve() && !view.empty_note.is_empty());
    assert!(journal_msg(&mut state, JournalMsg::ConflictResolve).is_empty());
}

#[test]
fn undo_from_the_tray_waits_for_a_running_session() {
    // Review A2: while a UAC prompt is up, the tray's undo only brings the window back.
    let mut state = app();
    update(
        &mut state,
        AppMsg::StartRequest {
            request: Request::Recover {
                apply: ApplyOptions::default(),
            },
            apply: ApplyOptions::default(),
        },
    );
    let effects = journal_msg(&mut state, JournalMsg::OpenUndo);
    assert_eq!(effects, vec![Effect::ShowWindow, Effect::Render]);
    assert_eq!(state.page, Page::Main);
    // Idle again: the undo preview opens (a request starts only from its button).
    state.session = SessionPhase::Idle;
    state.overlay = OverlayKind::None;
    let effects = journal_msg(&mut state, JournalMsg::OpenUndo);
    assert_eq!(state.page, Page::Recovery);
    assert_eq!(state.journal_pages.recovery.mode, RecoveryMode::Undo);
    assert!(started(&effects).is_none());
}

#[test]
fn a_restart_no_longer_needed_is_not_done() {
    let mut state = app();
    update(
        &mut state,
        read(
            journal(vec![restart_change(OpState::PendingReboot)]),
            BOOT,
            keychron_jis(),
        ),
    );
    update(&mut state, AppMsg::Navigate(Page::Restart));
    assert!(restart_view(&state).can_undo);
    journal_msg(&mut state, JournalMsg::RestartNow { acknowledged: true });
    // "後で" is off while the restart is under way.
    assert!(journal_msg(&mut state, JournalMsg::RestartLater).is_empty());
    // The I/O worker found the journal changed meanwhile: nothing restarted, read again.
    assert_eq!(
        journal_msg(&mut state, JournalMsg::RestartNotNeeded),
        vec![Effect::Read, Effect::Render]
    );
    assert!(!state.journal_pages.restart.restarting);
    assert_eq!(state.journal_pages.restart.error, None);
}

#[test]
fn a_hidden_start_shows_the_window_for_a_recovery_already_asked_about() {
    // `--tray` in a boot whose recovery prompt was shown before (design m3 F.2): the page does
    // not open again, but the window comes up with the banner.
    let written = entry("18181818-0000-4000-8000-000000000027", 36, OpState::Written);
    let mut state = app();
    state.settings.recovery.prompted.push(PromptedEntry {
        op: written.op_id.to_string(),
        boot: BOOT.to_text(),
    });
    let effects = update(
        &mut state,
        read(journal(vec![written]), BOOT, keychron_jis()),
    );
    assert_eq!(state.page, Page::Main);
    assert!(state.visible && effects.contains(&Effect::ShowWindow));
}
