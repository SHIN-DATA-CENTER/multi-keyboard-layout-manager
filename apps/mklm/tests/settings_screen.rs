//! Snapshots of the settings page, About and the restore preview of everything (design m3 B.14,
//! B.15, B.4, H.3) in Japanese and English: the sign-in start as the Run value shows it, the
//! longer countdown, "restore on uninstall" unread, unreadable, on and off, its confirmation
//! before the first UAC prompt and after it, and in an elevated GUI; the restore preview reads
//! the journal the development machine keeps from the M2 tests (schema 1).

mod common;

use mklm_core::{Journal, fixtures};
use mklm_gui::autostart::{AutostartReport, RunValue};
use mklm_gui::i18n::Lang;
use mklm_gui::settings::Settings;
use mklm_gui::state::{ChangeDraft, PreparedChange};
use mklm_gui::vm::SnapshotText;
use mklm_gui::vm::change::{
    ApplyMethod, ChangeContext, MethodDefault, change_page, plan_restore_scope, restore_all_members,
};
use mklm_gui::vm::settings::{SettingsInput, about_page, settings_page};

struct Scene {
    title: &'static str,
    settings: Settings,
    autostart: Option<AutostartReport>,
    restore_on_uninstall: Option<Result<bool, String>>,
    confirming: Option<bool>,
    elevated: bool,
}

impl Scene {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            settings: Settings::default(),
            autostart: None,
            restore_on_uninstall: None,
            confirming: None,
            elevated: false,
        }
    }

    fn text(&self, lang: Lang) -> String {
        let page = settings_page(&SettingsInput {
            settings: &self.settings,
            autostart: self.autostart.as_ref(),
            restore_on_uninstall: self.restore_on_uninstall.as_ref(),
            confirming: self.confirming,
            idle: true,
            elevated: self.elevated,
            lang,
        });
        format!("== {} ==\n{}", self.title, page.snapshot_text())
    }
}

fn registered(disabled_by_user: bool) -> AutostartReport {
    AutostartReport {
        state: Ok(RunValue {
            command_line: Some(r#""C:\Program Files\MKLM\mklm.exe" --tray"#.into()),
            disabled_by_user,
        }),
        write_error: None,
    }
}

fn scenes() -> Vec<Scene> {
    let opened = Scene::new("just opened: nothing read yet");

    let mut on = Scene::new("read: starts at sign-in, restores on uninstall");
    on.autostart = Some(registered(false));
    on.restore_on_uninstall = Some(Ok(true));

    let mut first = Scene::new("turning \"restore on uninstall\" off, before the first prompt");
    first.autostart = Some(registered(false));
    first.restore_on_uninstall = Some(Ok(true));
    first.confirming = Some(false);

    let mut later = Scene::new("turning it on again, longer countdown, hidden keyboards shown");
    later.settings.change.uac_notice_seen = true;
    later.settings.change.countdown_seconds = 60;
    later.settings.keyboards.show_hidden = true;
    later.restore_on_uninstall = Some(Ok(false));
    later.confirming = Some(true);

    let mut unreadable = Scene::new("sign-in start turned off in Task Manager, setting unreadable");
    unreadable.autostart = Some(registered(true));
    unreadable.restore_on_uninstall = Some(Err("access denied".into()));

    let mut elevated = Scene::new("an elevated MKLM: no prompt to explain");
    elevated.elevated = true;
    elevated.restore_on_uninstall = Some(Ok(true));
    elevated.confirming = Some(false);
    elevated.autostart = Some(AutostartReport {
        state: Ok(RunValue::default()),
        write_error: Some("access denied".into()),
    });

    vec![opened, on, first, later, unreadable, elevated]
}

/// "すべてのキーボードを MKLM 導入前に戻す…" on the development machine, with the journal it
/// keeps from the M2 tests (schema 1), after the preparation: as the machine is (nothing to put
/// back), and with the Keychron set to JIS since.
fn restore_everything(lang: Lang) -> String {
    let as_is = fixtures::dev_machine();
    let mut jis = fixtures::dev_machine();
    if let Some(keychron) = jis
        .keyboards
        .iter_mut()
        .find(|kb| kb.display_name == "Keychron Receiver")
    {
        keychron.overrides.keyboard_type_override = Some(7);
        keychron.overrides.keyboard_subtype_override = Some(2);
    }
    [("as it is", as_is), ("the Keychron set to JIS", jis)]
        .into_iter()
        .map(|(title, snapshot)| restore_preview(title, snapshot, lang))
        .collect::<Vec<_>>()
        .join("\n")
}

fn restore_preview(title: &str, snapshot: mklm_core::SystemSnapshot, lang: Lang) -> String {
    let (ops, baselines) = fixtures::schema_1_journal();
    let journal = Journal::parse(&ops, &baselines);
    let members = restore_all_members(&snapshot, &journal);
    let draft = ChangeDraft {
        restore: true,
        restore_all: true,
        plan: Some(plan_restore_scope(
            &snapshot,
            &journal,
            &mklm_core::RestoreScope::All,
            ApplyMethod::Restart,
        )),
        method_default: Some(MethodDefault {
            method: ApplyMethod::Restart,
            only_keyboard_warning: false,
        }),
        prepared: Some(PreparedChange {
            snapshot: snapshot.clone(),
            uncertain_values: false,
            warnings: Vec::new(),
            journal,
            blocker: None,
        }),
        ..ChangeDraft::new(String::new(), "all".into(), members)
    };
    let page = change_page(&ChangeContext {
        draft: &draft,
        display: Some(&snapshot),
        uac_notice_seen: true,
        elevated: false,
        seconds: 20,
        other_name: None,
        lang,
    });
    format!(
        "== restore everything (the M2 journal of the development machine), {title} ==\nkeyboards: {}\n{}",
        draft.members.len(),
        page.snapshot_text()
    )
}

fn all(lang: Lang) -> String {
    let mut parts: Vec<String> = scenes().iter().map(|scene| scene.text(lang)).collect();
    let about = about_page("0.1.0", "0123456789abcdef", lang);
    parts.push(format!("== about ==\n{}", about.snapshot_text()));
    parts.push(restore_everything(lang));
    parts.join("\n")
}

#[test]
fn the_settings_page_in_japanese() {
    common::assert_snapshot("settings.ja.txt", &all(Lang::Ja));
}

#[test]
fn the_settings_page_in_english() {
    common::assert_snapshot("settings.en.txt", &all(Lang::En));
}
