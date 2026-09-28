//! The wizard's flows (design m3 B.1, B.18) with fake reads and a fake helper: no window, no
//! registry, no UAC.

use std::time::Instant;

use mklm_client::orchestrator::{RequestEnd, RequestReport};
use mklm_client::run_once::RunOnceOutcome;
use mklm_client::session::SessionEnd;
use mklm_client::startup::summarize;
use mklm_core::{
    DeviceOverrides, Journal, JournalEntry, KeyboardType, Liveness, OpId, OpKind, OpState,
    OperationResult, Outcome, PendingAction, ProcessIdentity, SystemSnapshot, Timestamp, fixtures,
    value_names,
};

use super::*;
use crate::detect::scancode;
use crate::state::{SessionId, SessionOutcome, SystemRead, navigation_enabled};
use crate::vm::SnapshotText;
use crate::vm::test_journal::BOOT;

const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
const KEYCHRON_ROW: &str = "{F0D991EA-A583-5B9C-800D-48846AC6E633}";
const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
const CLEANUP_OP: &str = "5d6e7f80-1a2b-4c3d-8e9f-0a1b2c3d4e5f";

fn read_of(snapshot: SystemSnapshot, journal: Journal) -> AppMsg {
    let summary = summarize(&journal, BOOT, &|_: &ProcessIdentity| Liveness::Dead);
    AppMsg::SystemRead(Box::new(SystemRead {
        snapshot: Some(snapshot),
        warnings: Vec::new(),
        journal: Some(journal),
        boot: Some(BOOT),
        summary,
    }))
}

fn wizard_msg(state: &mut AppState, msg: WizardMsg) -> Vec<Effect> {
    update(state, AppMsg::Wizard(msg))
}

/// A first start: the wizard on its first step, the development machine read.
fn started(snapshot: SystemSnapshot) -> AppState {
    let mut state = AppState {
        lang: Some(Lang::Ja),
        page: Page::Wizard,
        wizard: Some(WizardState::default()),
        visible: true,
        ..AppState::default()
    };
    update(&mut state, read_of(snapshot, Journal::default()));
    state
}

/// From the welcome step to the summary; returns the effects of entering step 3.
fn to_keyboards(state: &mut AppState) -> Vec<Effect> {
    assert_eq!(
        wizard_msg(state, WizardMsg::Next),
        vec![Effect::Read, Effect::Render]
    );
    wizard_msg(state, WizardMsg::Next)
}

fn step(state: &AppState) -> WizardStep {
    state.wizard.as_ref().unwrap().step
}

fn page_of(state: &AppState) -> WizardPage {
    page(state)
}

fn started_request(effects: &[Effect]) -> Option<(SessionId, &Request)> {
    effects.iter().find_map(|effect| match effect {
        Effect::StartSession {
            session, request, ..
        } => Some((*session, request)),
        _ => None,
    })
}

fn ended(session: SessionId, result: OperationResult) -> AppMsg {
    AppMsg::SessionEnded {
        session,
        outcome: Box::new(SessionOutcome {
            report: RequestReport {
                first: RequestEnd::Ended(SessionEnd::Finished(result)),
                lost_needs_recovery: None,
                recovery: None,
                recovery_skipped: None,
            },
            run_once: Ok(RunOnceOutcome::NotNeeded),
        }),
    }
}

fn result(outcome: Outcome, op: Option<&str>, pending: Option<PendingAction>) -> OperationResult {
    OperationResult {
        op_id: op.map(|op| OpId::parse(op).unwrap()),
        outcome,
        failure: None,
        pending_action: pending,
        conflicts: Vec::new(),
        inv_ps2_violation: None,
        recovered: Vec::new(),
        warnings: Vec::new(),
    }
}

fn prepared_from(snapshot: SystemSnapshot) -> PreparedChange {
    PreparedChange {
        snapshot,
        uncertain_values: false,
        warnings: Vec::new(),
        journal: Journal::default(),
        blocker: None,
    }
}

fn token_of(effects: &[Effect]) -> u64 {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PrepareChange { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no preparation in {effects:?}"))
}

#[test]
fn skipping_completes_the_wizard_without_touching_the_sign_in_start() {
    let mut state = started(fixtures::dev_machine());
    assert!(!navigation_enabled(&state));
    let effects = wizard_msg(&mut state, WizardMsg::Skip);
    assert!(matches!(&effects[0], Effect::SaveSettings(s) if s.wizard.completed));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Autostart(_)))
    );
    assert_eq!(state.page, Page::Main);
    assert!(state.wizard.is_none());
    // Settings opens it again, at its first step.
    wizard_msg(&mut state, WizardMsg::Open);
    assert_eq!(
        (state.page, step(&state)),
        (Page::Wizard, WizardStep::Welcome)
    );
    assert_eq!(page_of(&state).heading, "手順 1 / 4: ようこそ");
}

#[test]
fn nothing_to_change_finishes_with_the_sign_in_start_on() {
    // T-START-5 without the window: four steps, nothing differs, "完了".
    let mut state = started(fixtures::dev_machine());
    let effects = to_keyboards(&mut state);
    // Step 3 reads the values of the non-keyboard collections once.
    assert!(matches!(
        effects.first(),
        Some(Effect::ReadNonKeyboardValues(keyboards)) if keyboards.len() == 4
    ));
    assert_eq!(step(&state), WizardStep::Keyboards);
    wizard_msg(&mut state, WizardMsg::ForeignRead(Ok(Vec::new())));
    assert_eq!(page_of(&state).keyboards.len(), 3);
    // Another read on the same step does not read them again.
    let effects = update(
        &mut state,
        read_of(fixtures::dev_machine(), Journal::default()),
    );
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::ReadNonKeyboardValues(_)))
    );
    assert_eq!(
        wizard_msg(&mut state, WizardMsg::Next),
        vec![Effect::Read, Effect::Render]
    );
    let summary = page_of(&state);
    assert_eq!(
        summary.body,
        "変更は不要です。どのキーボードも、選んだ配列のとおりに設定されています。"
    );
    assert!(summary.autostart_visible && summary.autostart && summary.can_next);
    assert_eq!(summary.next_text, "完了");
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    assert!(matches!(&effects[0], Effect::SaveSettings(s) if s.wizard.completed));
    assert_eq!(effects[1], Effect::Autostart(AutostartTask::Set(true)));
    assert_eq!(state.page, Page::Main);
    assert!(state.wizard.is_none() && navigation_enabled(&state));
}

#[test]
fn the_sign_in_start_can_be_left_off() {
    let mut state = started(fixtures::dev_machine());
    to_keyboards(&mut state);
    wizard_msg(&mut state, WizardMsg::Next);
    assert!(wizard_msg(&mut state, WizardMsg::Autostart(false)).is_empty());
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    assert!(effects.contains(&Effect::Autostart(AutostartTask::Set(false))));
}

#[test]
fn a_per_keyboard_change_goes_through_the_change_page_and_back() {
    let mut state = started(fixtures::dev_machine());
    to_keyboards(&mut state);
    wizard_msg(
        &mut state,
        WizardMsg::Choose {
            row: KEYCHRON_ROW.into(),
            index: 0,
        },
    );
    wizard_msg(&mut state, WizardMsg::Next);
    let summary = page_of(&state);
    assert_eq!(
        summary.lines,
        vec!["Keychron Receiver を JIS に".to_string()]
    );
    assert_eq!(summary.next_text, "Keychron Receiver の変更へ進む…");
    // The change page of WP-U3, JIS chosen and prepared.
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    assert_eq!(state.page, Page::Change);
    let draft = state.draft.as_ref().unwrap();
    assert!(draft.wizard);
    assert_eq!(draft.choice, Some(LayoutChoice::Jis));
    let token = token_of(&effects);
    // "キャンセル" goes back to the wizard, not to the main screen.
    update(&mut state, AppMsg::CancelChange);
    assert_eq!(state.page, Page::Wizard);
    assert_eq!(step(&state), WizardStep::Summary);
    // Again, to the end: the first prompt passes the UAC explanation, which goes back to the
    // change page when cancelled.
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    let token = {
        let again = token_of(&effects);
        assert!(again > token);
        again
    };
    update(
        &mut state,
        AppMsg::ChangePrepared {
            token,
            at: Instant::now(),
            prepared: Box::new(Ok(prepared_from(fixtures::dev_machine()))),
        },
    );
    assert_eq!(
        update(&mut state, AppMsg::ChangeApply),
        vec![Effect::Render]
    );
    assert_eq!(state.page, Page::UacNotice);
    let effects = update(&mut state, AppMsg::UacGo);
    let (session, request) = started_request(&effects).unwrap();
    assert!(matches!(request, Request::SetLayout(set) if set.instance_id == KEYCHRON));
    // The result shows over the wizard; the next read shows the Keychron set to JIS.
    update(
        &mut state,
        ended(
            session,
            result(
                Outcome::Confirmed,
                Some("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"),
                None,
            ),
        ),
    );
    assert_eq!(
        (state.page, state.overlay),
        (Page::Wizard, OverlayKind::Result)
    );
    assert!(state.wizard.as_ref().unwrap().ran);
    assert_eq!(
        update(&mut state, AppMsg::ResultClosed),
        vec![Effect::Read, Effect::Render]
    );
    let mut after = fixtures::dev_machine();
    after.keyboards[1].overrides.keyboard_type_override = Some(7);
    after.keyboards[1].overrides.keyboard_subtype_override = Some(2);
    after.keyboards[1].reported_type = Some(KeyboardType::JIS);
    update(&mut state, read_of(after, Journal::default()));
    let summary = page_of(&state);
    assert_eq!(summary.body, "設定が終わりました。");
    assert_eq!(summary.next_text, "完了");
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    assert!(effects.contains(&Effect::Autostart(AutostartTask::Set(true))));
    assert_eq!(state.page, Page::Main);
}

#[test]
fn a_kept_us_keyboard_does_not_leave_the_wizard_for_the_ime_guide() {
    let mut state = started(fixtures::dev_machine());
    state.settings.change.uac_notice_seen = true;
    to_keyboards(&mut state);
    wizard_msg(
        &mut state,
        WizardMsg::Choose {
            row: BUILT_IN.into(),
            index: 1,
        },
    );
    wizard_msg(&mut state, WizardMsg::Next);
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    update(
        &mut state,
        AppMsg::ChangePrepared {
            token: token_of(&effects),
            at: Instant::now(),
            prepared: Box::new(Ok(prepared_from(fixtures::dev_machine()))),
        },
    );
    let effects = update(&mut state, AppMsg::ChangeApply);
    let (session, _) = started_request(&effects).unwrap();
    update(
        &mut state,
        ended(
            session,
            result(
                Outcome::Confirmed,
                Some("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"),
                None,
            ),
        ),
    );
    let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
    // The note stays; the guide button that would end the wizard does not.
    assert!(!shown.ime_note.is_empty());
    assert_eq!(shown.next, None);
}

/// A fixed-JIS PC with a new keyboard without values (the Keychron).
fn fixed_jis() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    snapshot.global = fixtures::global_fixed_jis();
    snapshot.keyboards[1].overrides = DeviceOverrides::default();
    snapshot.keyboards[1].reported_type = Some(KeyboardType::JIS);
    snapshot
}

#[test]
fn the_migration_is_prepared_sent_with_its_assignment_and_leads_to_the_restart() {
    let mut state = started(fixed_jis());
    to_keyboards(&mut state);
    wizard_msg(
        &mut state,
        WizardMsg::Choose {
            row: KEYCHRON_ROW.into(),
            index: 1,
        },
    );
    wizard_msg(&mut state, WizardMsg::Next);
    // The read that follows entering the summary prepares the migration.
    let effects = update(&mut state, read_of(fixed_jis(), Journal::default()));
    let token = token_of(&effects);
    let preparing = page_of(&state);
    assert!(preparing.busy && !preparing.can_next);
    // Nothing starts before it is prepared.
    assert!(wizard_msg(&mut state, WizardMsg::Next).is_empty());
    update(
        &mut state,
        AppMsg::ChangePrepared {
            token,
            at: Instant::now(),
            prepared: Box::new(Ok(prepared_from(fixed_jis()))),
        },
    );
    let summary = page_of(&state);
    assert!(summary.can_next && summary.migration);
    assert_eq!(
        summary.lines,
        vec![
            "キーボードごとモードへ移行します（PC の標準配列 JIS）。PC の再起動が 1 回必要です。"
                .to_string(),
            "Keychron Receiver を US に（移行と同時に書きます）".to_string(),
        ]
    );
    // The standard layout can be chosen for the migration only.
    wizard_msg(&mut state, WizardMsg::Standard(1));
    assert_eq!(page_of(&state).standard_selected, Some(1));
    wizard_msg(&mut state, WizardMsg::Standard(0));
    // The first request: the UAC explanation first; cancelled, back to the wizard.
    assert_eq!(
        wizard_msg(&mut state, WizardMsg::Next),
        vec![Effect::Render]
    );
    assert_eq!(state.page, Page::UacNotice);
    update(&mut state, AppMsg::CancelChange);
    assert_eq!(state.page, Page::Wizard);
    wizard_msg(&mut state, WizardMsg::Next);
    let effects = update(&mut state, AppMsg::UacGo);
    assert!(matches!(&effects[0], Effect::SaveSettings(s) if s.change.uac_notice_seen));
    let (session, request) = started_request(&effects).unwrap();
    let Request::Migrate(migrate) = request else {
        panic!("{request:?}")
    };
    assert_eq!(migrate.standard, Layout::Jis);
    assert_eq!(
        migrate.assignments,
        vec![Assignment {
            instance_id: KEYCHRON.into(),
            layout: LayoutChoice::Us,
        }]
    );
    assert_eq!(
        migrate.expected.as_ref().unwrap().apply,
        Some(PendingAction::RestartPc)
    );
    assert_eq!(state.page, Page::Wizard);
    update(
        &mut state,
        ended(
            session,
            result(
                Outcome::PendingReboot,
                Some("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"),
                Some(PendingAction::RestartPc),
            ),
        ),
    );
    // The result's next step: the restart page; the wizard has done its part.
    let effects = update(&mut state, AppMsg::ResultNextStep);
    assert!(matches!(&effects[0], Effect::SaveSettings(s) if s.wizard.completed));
    assert_eq!(effects[1], Effect::Autostart(AutostartTask::Set(true)));
    assert_eq!(state.page, Page::Restart);
    assert!(state.wizard.is_none());
}

fn cleanup_waiting() -> Journal {
    let entry = JournalEntry {
        schema_version: 2,
        op_id: OpId::parse(CLEANUP_OP).unwrap(),
        seq: 1,
        kind: OpKind::Cleanup {
            instance_id: BUILT_IN.into(),
            names: vec![
                value_names::HID_TYPE.into(),
                value_names::HID_SUBTYPE.into(),
            ],
        },
        state: OpState::AwaitingConfirm,
        boot_id: BOOT,
        owner: ProcessIdentity {
            pid: 1,
            creation_time: 1,
        },
        created_at: Timestamp(1_790_000_000_000),
        updated_at: Timestamp(1_790_000_000_000),
        apply: None,
        countdown: None,
        records: Vec::new(),
        context: Vec::new(),
        failure: None,
        revert_mode: None,
        apply_pending: None,
        history: Vec::new(),
    };
    Journal {
        entries: vec![entry],
        ..Journal::default()
    }
}

/// The PS/2 keyboard with HID names its driver never reads.
fn with_unread_values() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    snapshot.keyboards[0].overrides.keyboard_type_override = Some(4);
    snapshot.keyboards[0].overrides.keyboard_subtype_override = Some(0);
    snapshot
}

#[test]
fn a_cleanup_is_sent_then_kept_or_undone_before_anything_else() {
    let mut state = started(with_unread_values());
    state.settings.change.uac_notice_seen = true;
    to_keyboards(&mut state);
    let rows = page_of(&state).keyboards;
    assert!(rows[0].problem_deletable);
    wizard_msg(
        &mut state,
        WizardMsg::Problem {
            row: BUILT_IN.into(),
            delete: true,
        },
    );
    wizard_msg(
        &mut state,
        WizardMsg::Choose {
            row: KEYCHRON_ROW.into(),
            index: 0,
        },
    );
    wizard_msg(&mut state, WizardMsg::Next);
    update(
        &mut state,
        read_of(with_unread_values(), Journal::default()),
    );
    let summary = page_of(&state);
    assert_eq!(summary.lines.len(), 2);
    assert!(summary.lines[0].contains("ドライバーが読まない値を削除"));
    assert_eq!(
        summary.next_text,
        "値を削除する（次に Windows の確認が出ます）"
    );
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    let (session, request) = started_request(&effects).unwrap();
    assert_eq!(
        request,
        &Request::CleanupValues {
            instance_id: BUILT_IN.into(),
            names: vec![
                value_names::HID_TYPE.into(),
                value_names::HID_SUBTYPE.into()
            ],
        }
    );
    update(
        &mut state,
        ended(
            session,
            result(Outcome::AwaitingConfirm, Some(CLEANUP_OP), None),
        ),
    );
    update(&mut state, AppMsg::ResultClosed);
    // The values are gone; the cleanup waits for keep or undo, and blocks the rest meanwhile.
    update(
        &mut state,
        read_of(fixtures::dev_machine(), cleanup_waiting()),
    );
    let summary = page_of(&state);
    assert!(
        summary
            .body
            .starts_with("日本語 PS/2 キーボード (106/109 キー Ctrl+英数) の値を削除しました")
    );
    assert_eq!(summary.secondary_text, "元に戻す");
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    let (session, request) = started_request(&effects).unwrap();
    assert_eq!(
        request,
        &Request::Confirm {
            op_id: OpId::parse(CLEANUP_OP).unwrap()
        }
    );
    update(
        &mut state,
        ended(session, result(Outcome::Confirmed, Some(CLEANUP_OP), None)),
    );
    update(&mut state, AppMsg::ResultClosed);
    // "元に戻す" would have sent a revert instead.
    let mut undo = started(fixtures::dev_machine());
    undo.settings.change.uac_notice_seen = true;
    undo.wizard.as_mut().unwrap().step = WizardStep::Summary;
    update(
        &mut undo,
        read_of(fixtures::dev_machine(), cleanup_waiting()),
    );
    let effects = wizard_msg(&mut undo, WizardMsg::Secondary);
    assert!(matches!(
        started_request(&effects),
        Some((_, Request::Revert { op_id, .. })) if op_id.as_str() == CLEANUP_OP
    ));
    // Kept: the layout change is next.
    update(
        &mut state,
        read_of(fixtures::dev_machine(), Journal::default()),
    );
    assert_eq!(page_of(&state).next_text, "Keychron Receiver の変更へ進む…");
}

#[test]
fn a_blocked_journal_leaves_the_rest_for_later() {
    let mut state = started(fixtures::dev_machine());
    to_keyboards(&mut state);
    wizard_msg(
        &mut state,
        WizardMsg::Choose {
            row: KEYCHRON_ROW.into(),
            index: 0,
        },
    );
    wizard_msg(&mut state, WizardMsg::Next);
    // A change of another front end waits for the restart.
    let mut waiting = crate::vm::test_journal::entry(
        "12121212-0000-4000-8000-000000000021",
        31,
        OpState::PendingReboot,
    );
    waiting.apply = Some(PendingAction::RestartPc);
    update(
        &mut state,
        read_of(
            fixtures::dev_machine(),
            Journal {
                entries: vec![waiting],
                ..Journal::default()
            },
        ),
    );
    let summary = page_of(&state);
    assert_eq!(summary.body, "次の変更が残っていますが、今は行えません。");
    assert_eq!(summary.next_text, "完了");
    assert!(
        summary
            .notes
            .iter()
            .any(|note| note.ends_with("残りの変更は、後でメイン画面の［変更…］から行えます。")),
        "{:?}",
        summary.notes
    );
    // "完了" starts nothing.
    let effects = wizard_msg(&mut state, WizardMsg::Next);
    assert!(started_request(&effects).is_none());
    assert_eq!(state.page, Page::Main);
}

#[test]
fn the_detection_on_step_three_learns_and_selects() {
    let mut state = started(fixtures::dev_machine());
    to_keyboards(&mut state);
    let key = |state: &mut AppState, id: &str, code: u32| {
        update(
            state,
            AppMsg::DeviceKey {
                instance_id: id.into(),
                scancode: code,
                at: Instant::now(),
            },
        )
    };
    // A Tab does not fix the keyboard; the Keychron's answer keys do.
    key(&mut state, BUILT_IN, 0x0F);
    assert_eq!(state.wizard.as_ref().unwrap().detection.device, None);
    key(&mut state, KEYCHRON, scancode::YEN);
    let effects = key(&mut state, KEYCHRON, scancode::RO);
    assert!(matches!(effects[0], Effect::SaveSettings(_)));
    assert_eq!(
        state.settings.physical(KEYCHRON).map(|p| p.layout),
        Some(PhysicalKind::Jis)
    );
    let shown = page_of(&state);
    assert_eq!(
        shown.detect.verdict,
        "✓ Keychron Receiver は JIS 配列のキーボードです"
    );
    assert_eq!(shown.detect.choose_text, "JIS を選ぶ");
    wizard_msg(&mut state, WizardMsg::UseDetected);
    let shown = page_of(&state);
    assert_eq!(shown.keyboards[1].choice, Some(Layout::Jis));
    assert_eq!(shown.detect.choose_text, "");
    wizard_msg(&mut state, WizardMsg::RestartDetection);
    assert_eq!(page_of(&state).detect.verdict, "");
    // Not on another step.
    wizard_msg(&mut state, WizardMsg::Back);
    assert!(key(&mut state, KEYCHRON, scancode::EQUAL).is_empty());
}

#[test]
fn the_wizard_opens_by_navigation_and_closes_when_left() {
    let mut state = AppState {
        lang: Some(Lang::En),
        visible: true,
        ..AppState::default()
    };
    update(&mut state, AppMsg::Navigate(Page::Wizard));
    assert_eq!(state.page, Page::Wizard);
    assert!(state.wizard.is_some());
    let snapshot = page_of(&state).snapshot_text();
    assert!(
        snapshot.starts_with("heading: Step 1 of 4: Welcome\n"),
        "{snapshot}"
    );
    update(&mut state, AppMsg::Navigate(Page::Main));
    assert!(state.wizard.is_none());
    // Its messages do nothing then.
    assert!(wizard_msg(&mut state, WizardMsg::Next).is_empty());
    assert!(wizard_msg(&mut state, WizardMsg::ForeignRead(Ok(Vec::new()))).is_empty());
    // A running session keeps it closed.
    state.session = SessionPhase::Launching { id: 3 };
    assert!(wizard_msg(&mut state, WizardMsg::Open).is_empty());
}
