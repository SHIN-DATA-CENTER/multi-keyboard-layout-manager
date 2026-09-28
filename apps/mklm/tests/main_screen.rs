//! Snapshots of the main screen (design m3 B.2, B.12, H.3) in Japanese and English: the status
//! line, the banner and its button, the rows with their badges, hints and screen-reader text and
//! the identification announcement, for the development machine and the journal states the
//! screen has to explain — and the "今すぐ反映…" page it leads to. The journal entries are
//! schema-1 JSON, as M2 wrote them on the development machine; one scene reads the journal the
//! development machine keeps from the M2 tests.

mod common;

use mklm_client::startup::{StartupSummary, summarize};
use mklm_core::{BootId, Journal, JournalEntry, Liveness, SystemSnapshot, fixtures};
use mklm_gui::i18n::Lang;
use mklm_gui::settings::Settings;
use mklm_gui::state::ChangeDraft;
use mklm_gui::vm::SnapshotText;
use mklm_gui::vm::change::{ApplyMethod, MethodDefault};
use mklm_gui::vm::keyboards::{ApplyNowContext, MainInput, apply_now_page, main_screen};

const BOOT: &str = "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d";
const EARLIER_BOOT: &str = "11112222-3333-4444-5555-666677778888";
const KEYCHRON: &str = r"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000";

/// A schema-1 entry that set the Keychron to JIS, in `state`, written in `boot`, with
/// `apply_pending` (JSON or `null`).
fn keychron_entry(state: &str, boot: &str, apply_pending: &str) -> JournalEntry {
    let json = format!(
        r#"{{
          "schema_version": 1,
          "op_id": "0000000a-0000-4000-8000-000000000000",
          "seq": 1,
          "kind": {{ "kind": "set-layout", "requested": "{KEYCHRON}",
            "instance_ids": ["{KEYCHRON}"], "layout": "jis" }},
          "state": "{state}",
          "boot_id": "{boot}",
          "owner": {{ "pid": 12345, "creation_time": 134036790000000000 }},
          "created_at": 1790500000000,
          "updated_at": 1790500004000,
          "apply": "reset-keyboard",
          "countdown": null,
          "records": [],
          "context": [],
          "failure": null,
          "apply_pending": {apply_pending},
          "history": []
        }}"#
    );
    JournalEntry::from_json(&json).unwrap()
}

fn reset_pending() -> String {
    format!(
        r#"{{ "action": "reset-keyboard", "instance_ids": ["{KEYCHRON}"], "since": "{BOOT}" }}"#
    )
}

/// The Keychron stored as JIS while it still types US.
fn keychron_saved_as_jis() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    let keychron = snapshot
        .keyboards
        .iter_mut()
        .find(|kb| kb.display_name == "Keychron Receiver")
        .unwrap();
    keychron.overrides.keyboard_type_override = Some(7);
    keychron.overrides.keyboard_subtype_override = Some(2);
    snapshot
}

struct Scene {
    title: &'static str,
    snapshot: SystemSnapshot,
    journal: Journal,
    summary: StartupSummary,
    settings: Settings,
    highlighted: Option<String>,
    active_hkl: u32,
}

impl Scene {
    fn new(title: &'static str, snapshot: SystemSnapshot, entries: Vec<JournalEntry>) -> Self {
        Self::with_journal(
            title,
            snapshot,
            Journal {
                entries,
                ..Journal::default()
            },
        )
    }

    fn with_journal(title: &'static str, snapshot: SystemSnapshot, journal: Journal) -> Self {
        let summary = summarize(&journal, boot(), &|_| Liveness::Dead);
        Self {
            title,
            snapshot,
            journal,
            summary,
            settings: Settings::default(),
            highlighted: None,
            active_hkl: 0x0411_0411,
        }
    }

    fn text(&self, lang: Lang) -> String {
        let screen = main_screen(&MainInput {
            snapshot: &self.snapshot,
            journal: Some((&self.journal, boot())),
            summary: &self.summary,
            settings: &self.settings,
            active_hkl: self.active_hkl,
            highlighted: self.highlighted.as_deref(),
            lang,
        });
        format!("== {} ==\n{}", self.title, screen.snapshot_text())
    }
}

fn boot() -> BootId {
    BootId::parse(BOOT).unwrap()
}

fn scenes() -> Vec<Scene> {
    let (ops, baselines) = fixtures::schema_1_journal();
    let dev_journal = Scene::with_journal(
        "the development machine with its journal from the M2 tests (schema 1)",
        fixtures::dev_machine(),
        Journal::parse(&ops, &baselines),
    );

    let mut identify = Scene::new("identifying", fixtures::dev_machine(), Vec::new());
    identify
        .settings
        .keyboards
        .seen
        .push(fixtures::keychron().instance_id);
    identify.highlighted = Some(fixtures::keychron().instance_id);

    let apply_now = Scene::new(
        "a saved reset, offered as \"apply now\"",
        keychron_saved_as_jis(),
        vec![keychron_entry("confirmed", BOOT, &reset_pending())],
    );

    let waiting = Scene::new(
        "a change waits for the PC restart",
        keychron_saved_as_jis(),
        vec![keychron_entry("pending-reboot", BOOT, "null")],
    );

    let post_reboot = Scene::new(
        "after the restart",
        fixtures::dev_machine(),
        vec![keychron_entry("pending-reboot", EARLIER_BOOT, "null")],
    );

    let mut unreadable = Scene::new("an unreadable journal", fixtures::dev_machine(), Vec::new());
    unreadable.summary.unreadable = 1;

    let mut fixed = fixtures::dev_machine();
    fixed.global = fixtures::global_fixed_jis();
    fixed.input.sign_in_preload = vec!["00000409".into()];
    let mut hidden = Scene::new(
        "fixed mode, English input, hidden and disconnected shown",
        fixed,
        Vec::new(),
    );
    hidden.active_hkl = 0x0409_0409;
    hidden.settings.keyboards.show_hidden = true;
    hidden
        .settings
        .keyboards
        .hidden
        .push(fixtures::vxe_ble().container_id.unwrap());

    vec![
        dev_journal,
        identify,
        apply_now,
        waiting,
        post_reboot,
        unreadable,
        hidden,
    ]
}

/// The "今すぐ反映…" page (the change page of WP-U3 with only the apply method): the first time,
/// when only that keyboard typed, and after it took effect meanwhile.
fn apply_now_pages(lang: Lang) -> String {
    let draft = |method: Option<ApplyMethod>, default: MethodDefault| ChangeDraft {
        apply_now: true,
        method,
        method_default: Some(default),
        ..ChangeDraft::new(
            String::new(),
            "Keychron Receiver".into(),
            vec![fixtures::keychron().instance_id],
        )
    };
    let live = draft(
        None,
        MethodDefault {
            method: ApplyMethod::Live,
            only_keyboard_warning: false,
        },
    );
    let only = draft(
        None,
        MethodDefault {
            method: ApplyMethod::Restart,
            only_keyboard_warning: true,
        },
    );
    [
        ("apply now, the first time", &live, true, false),
        ("apply now, only that keyboard typed", &only, true, true),
        ("apply now, put into effect meanwhile", &live, false, true),
    ]
    .into_iter()
    .map(|(title, draft, offered, uac_notice_seen)| {
        let page = apply_now_page(&ApplyNowContext {
            draft,
            offered,
            blocked: None,
            uac_notice_seen,
            elevated: false,
            lang,
        });
        format!("== {title} ==\n{}", page.snapshot_text())
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn all(lang: Lang) -> String {
    let mut parts: Vec<String> = scenes().iter().map(|scene| scene.text(lang)).collect();
    parts.push(apply_now_pages(lang));
    parts.join("\n")
}

#[test]
fn the_main_screen_in_japanese() {
    common::assert_snapshot("main.ja.txt", &all(Lang::Ja));
}

#[test]
fn the_main_screen_in_english() {
    common::assert_snapshot("main.en.txt", &all(Lang::En));
}
