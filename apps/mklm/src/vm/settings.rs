//! The settings page and About (design m3 B.14, B.15; WP-U7) as view-models, and what one setting
//! changes in every request: the keep-or-revert time (design m3 B.6, WP-E3).
//!
//! "アンインストール時にキーボードの設定を元に戻す" is machine-wide (plan 3.13): the page shows
//! the value read unelevated; "変更…" opens a confirmation with what the new value means and what
//! the Windows prompt looks like — the whole explanation before this user's first prompt, one line
//! after it (review U14) — and only the confirmation's button starts the helper (design m3 0.2
//! principle 6).

use mklm_core::ApplyOptions;
use mklm_ipc::Request;

use super::SnapshotText;
use crate::autostart::{self, AutostartReport};
use crate::i18n::{self, Lang, UninstallRestore as Known};
use crate::settings::Settings;

/// Everything the settings page depends on.
#[derive(Debug, Clone, Copy)]
pub struct SettingsInput<'a> {
    pub settings: &'a Settings,
    /// The autostart value as the I/O worker last read it (design m3 F.3).
    pub autostart: Option<&'a AutostartReport>,
    /// "restore on uninstall" as last read: `None` until read, `Err` (English reason) when it
    /// could not be read.
    pub restore_on_uninstall: Option<&'a Result<bool, String>>,
    /// The confirmation is open, for this new value.
    pub confirming: Option<bool>,
    /// No helper session runs.
    pub idle: bool,
    /// The GUI runs elevated: no UAC prompt follows, so none is explained (design m3 B.5).
    pub elevated: bool,
    pub lang: Lang,
}

/// The settings page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsPage {
    /// The sign-in start: the Run value itself (design m3 F.3).
    pub autostart_on: bool,
    pub autostart_available: bool,
    pub autostart_note: String,
    /// "確認の時間を長くする（60 秒）".
    pub long_countdown: bool,
    /// "非表示と未接続のキーボードも表示する".
    pub show_hidden: bool,
    pub uninstall: UninstallView,
}

/// "アンインストール時にキーボードの設定を元に戻す".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UninstallView {
    pub status: String,
    pub note: String,
    /// "変更…" can be pressed: the value is known and no helper session runs.
    pub can_change: bool,
    pub confirm: Option<UninstallConfirm>,
}

/// The confirmation after "変更…".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UninstallConfirm {
    /// The value the button saves.
    pub on: bool,
    pub title: String,
    pub text: String,
    /// Before this user's first UAC prompt: the whole explanation of the prompt (the one of the
    /// standalone page before the first change, M2 R12).
    pub uac_explanation: bool,
    /// After it: the one line (review U14). Empty when elevated.
    pub uac_line: String,
    pub button: String,
}

/// What the page knows of the machine-wide value.
fn known(value: Option<&Result<bool, String>>) -> Known {
    match value {
        None => Known::Reading,
        Some(Err(_)) => Known::Unreadable,
        Some(Ok(true)) => Known::On,
        Some(Ok(false)) => Known::Off,
    }
}

/// The settings page (design m3 B.14).
pub fn settings_page(input: &SettingsInput<'_>) -> SettingsPage {
    let lang = input.lang;
    let switch = autostart::view(input.autostart);
    let value = known(input.restore_on_uninstall);
    let current = match value {
        Known::On => Some(true),
        Known::Off => Some(false),
        Known::Reading | Known::Unreadable => None,
    };
    let prompt = !input.elevated;
    let confirm = input
        .confirming
        .filter(|on| current == Some(!on))
        .map(|on| UninstallConfirm {
            on,
            title: i18n::uninstall_confirm_title(on, lang),
            text: i18n::uninstall_confirm_text(on, lang),
            uac_explanation: prompt && !input.settings.change.uac_notice_seen,
            uac_line: if prompt && input.settings.change.uac_notice_seen {
                i18n::uac_line(lang)
            } else {
                String::new()
            },
            button: i18n::uninstall_confirm_button(on, prompt, lang),
        });
    SettingsPage {
        autostart_on: switch.on,
        autostart_available: switch.available,
        autostart_note: switch
            .note
            .map(|note| i18n::autostart_note(note, lang))
            .unwrap_or_default(),
        long_countdown: crate::vm::change::countdown_seconds(input.settings) == 60,
        show_hidden: input.settings.keyboards.show_hidden,
        uninstall: UninstallView {
            status: i18n::uninstall_restore_status(value, lang),
            note: i18n::uninstall_restore_note(value, lang),
            can_change: current.is_some() && input.idle,
            confirm,
        },
    }
}

impl SnapshotText for SettingsPage {
    fn snapshot_text(&self) -> String {
        let flag = |on: bool| if on { "☑" } else { "☐" };
        let mut out = format!(
            "autostart: {}\nautostart available: {}\n",
            flag(self.autostart_on),
            self.autostart_available
        );
        if !self.autostart_note.is_empty() {
            out.push_str(&format!("autostart note: {}\n", self.autostart_note));
        }
        out.push_str(&format!(
            "long countdown: {}\nshow hidden: {}\nuninstall: {}\nuninstall note: {}\ncan change: {}\n",
            flag(self.long_countdown),
            flag(self.show_hidden),
            self.uninstall.status,
            self.uninstall.note,
            self.uninstall.can_change
        ));
        if let Some(confirm) = &self.uninstall.confirm {
            out.push_str(&format!(
                "confirm: {}\nconfirm text: {}\n",
                confirm.title, confirm.text
            ));
            if confirm.uac_explanation {
                out.push_str("uac: (the whole explanation)\n");
            }
            if !confirm.uac_line.is_empty() {
                out.push_str(&format!("uac: {}\n", confirm.uac_line));
            }
            out.push_str(&format!("button: {}\n", confirm.button));
        }
        out
    }
}

/// About (design m3 B.15).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AboutPage {
    /// "バージョン 0.1.0（ビルド …）".
    pub version: String,
    /// The main third-party components with their licences.
    pub licences: Vec<String>,
}

pub fn about_page(version: &str, build_id: &str, lang: Lang) -> AboutPage {
    AboutPage {
        version: i18n::about_version(version, build_id, lang),
        licences: i18n::third_party_components(lang),
    }
}

impl SnapshotText for AboutPage {
    fn snapshot_text(&self) -> String {
        let mut out = format!("version: {}\n", self.version);
        for line in &self.licences {
            out.push_str(&format!("licence: {line}\n"));
        }
        out
    }
}

/// `request` and `apply` with the keep-or-revert time `seconds` (design m3 B.6, B.14; WP-E3): in
/// every `ApplyOptions` the request carries, and in the options the recovery after a lost helper
/// uses. Only 20 and 60 are sent (the helper refuses anything else); another value counts as 20.
pub fn with_countdown(
    mut request: Request,
    mut apply: ApplyOptions,
    seconds: u32,
) -> (Request, ApplyOptions) {
    let seconds = if mklm_core::COUNTDOWN_SECONDS_CHOICES.contains(&seconds) {
        seconds
    } else {
        mklm_core::DEFAULT_COUNTDOWN_SECONDS
    };
    apply.countdown_seconds = seconds;
    match &mut request {
        Request::SetLayout(set) => set.apply.countdown_seconds = seconds,
        Request::RestoreBaseline(restore) => restore.apply.countdown_seconds = seconds,
        Request::ResolveConflict(resolve) => resolve.apply.countdown_seconds = seconds,
        Request::Revert { apply, .. } | Request::Recover { apply } | Request::Undo { apply } => {
            apply.countdown_seconds = seconds;
        }
        // Nothing of these counts down: a migration takes effect at the restart, a confirmation
        // or a cleanup waits for the user without a countdown, a machine setting resets nothing.
        Request::Migrate(_)
        | Request::Confirm { .. }
        | Request::CleanupValues { .. }
        | Request::SetMachineSettings { .. } => {}
    }
    (request, apply)
}

#[cfg(test)]
mod tests {
    use mklm_core::{ConflictPolicy, LayoutChoice, OpId, RestoreScope};
    use mklm_ipc::{RestoreBaselineRequest, SetLayoutRequest};

    use super::*;
    use crate::autostart::RunValue;

    fn input<'a>(
        settings: &'a Settings,
        value: Option<&'a Result<bool, String>>,
        lang: Lang,
    ) -> SettingsInput<'a> {
        SettingsInput {
            settings,
            autostart: None,
            restore_on_uninstall: value,
            confirming: None,
            idle: true,
            elevated: false,
            lang,
        }
    }

    #[test]
    fn the_uninstall_setting_changes_only_when_it_is_known() {
        let settings = Settings::default();
        // Not read yet, or unreadable: "変更…" is off (the new value would be a guess).
        for value in [None, Some(Err("access denied".to_string()))] {
            let page = settings_page(&input(&settings, value.as_ref(), Lang::Ja));
            assert!(!page.uninstall.can_change, "{value:?}");
            assert!(page.uninstall.status.ends_with("不明"));
        }
        let on = Ok(true);
        let page = settings_page(&input(&settings, Some(&on), Lang::Ja));
        assert!(page.uninstall.can_change);
        assert_eq!(
            page.uninstall.status,
            "アンインストール時にキーボードの設定を元に戻す: オン"
        );
        // During a helper session nothing more is started.
        let busy = SettingsInput {
            idle: false,
            ..input(&settings, Some(&on), Lang::Ja)
        };
        assert!(!settings_page(&busy).uninstall.can_change);
    }

    /// The confirmation to turn "restore on uninstall" off.
    fn confirming(settings: &Settings, elevated: bool) -> UninstallConfirm {
        let on = Ok(true);
        settings_page(&SettingsInput {
            confirming: Some(false),
            elevated,
            ..input(settings, Some(&on), Lang::Ja)
        })
        .uninstall
        .confirm
        .unwrap()
    }

    #[test]
    fn the_confirmation_explains_the_first_prompt_in_full() {
        let mut settings = Settings::default();
        let on = Ok(true);
        let first = confirming(&settings, false);
        assert!(!first.on && first.uac_explanation && first.uac_line.is_empty());
        assert_eq!(first.button, "オフにする（次に Windows の確認が出ます）");
        settings.change.uac_notice_seen = true;
        let later = confirming(&settings, false);
        assert!(!later.uac_explanation && !later.uac_line.is_empty());
        // Elevated: no prompt follows, none is explained.
        let elevated = confirming(&settings, true);
        assert!(!elevated.uac_explanation && elevated.uac_line.is_empty());
        assert_eq!(elevated.button, "オフにする");
        // A confirmation for the value that is set already is not shown (read again meanwhile).
        let stale = settings_page(&SettingsInput {
            confirming: Some(true),
            ..input(&settings, Some(&on), Lang::Ja)
        });
        assert!(stale.uninstall.confirm.is_none());
    }

    #[test]
    fn switches_follow_the_settings_and_the_run_value() {
        let mut settings = Settings::default();
        settings.change.countdown_seconds = 60;
        settings.keyboards.show_hidden = true;
        let report = AutostartReport {
            state: Ok(RunValue {
                command_line: Some(r#""C:\MKLM\mklm.exe" --tray"#.into()),
                disabled_by_user: true,
            }),
            write_error: None,
            elevated: false,
        };
        let page = settings_page(&SettingsInput {
            autostart: Some(&report),
            ..input(&settings, None, Lang::En)
        });
        assert!(page.long_countdown && page.show_hidden);
        // Turned off in Task Manager: shown off and unavailable, with the reason (design m3 F.3).
        assert!(!page.autostart_on && !page.autostart_available);
        assert!(page.autostart_note.starts_with("Turned off"));
    }

    #[test]
    fn japanese_settings_and_about_use_only_allowed_latin_words() {
        let mut settings = Settings::default();
        settings.change.uac_notice_seen = true;
        let mut texts = Vec::new();
        for value in [Ok(true), Ok(false), Err("x".to_string())] {
            for confirming in [None, Some(true), Some(false)] {
                let page = settings_page(&SettingsInput {
                    confirming,
                    ..input(&settings, Some(&value), Lang::Ja)
                });
                texts.push(page.snapshot_text());
            }
        }
        texts.push(crate::i18n::uninstall_restore_saved(true, Lang::Ja));
        texts.push(crate::i18n::uninstall_restore_saved(false, Lang::Ja));
        texts.push(crate::i18n::restore_all_title(Lang::Ja));
        texts.push(about_page("0.1.0", "0123abcd4567ef89", Lang::Ja).version);
        for text in &texts {
            let values = crate::vm::snapshot_values(text);
            assert!(
                crate::vm::unexpected_latin(&values, &[]).is_empty(),
                "{values}: {:?}",
                crate::vm::unexpected_latin(&values, &[])
            );
        }
        // The components keep their own names in both languages.
        let about = about_page("0.1.0", "0123abcd4567ef89", Lang::Ja);
        assert_eq!(about.licences.len(), 4);
        assert!(about.licences[0].starts_with("Slint: "));
    }

    #[test]
    fn the_countdown_setting_reaches_every_request_that_can_count_down() {
        let apply = ApplyOptions {
            allow_live_reset: true,
            other_input_available: true,
            ..ApplyOptions::default()
        };
        let set = Request::SetLayout(SetLayoutRequest {
            instance_id: "X".into(),
            layout: LayoutChoice::Jis,
            apply,
            expected: None,
        });
        let (request, options) = with_countdown(set, apply, 60);
        assert_eq!(options.countdown_seconds, 60);
        assert!(
            matches!(request, Request::SetLayout(set) if set.apply.countdown_seconds == 60
            && set.apply.allow_live_reset)
        );
        let op_id = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        for request in [
            Request::Revert { op_id, apply },
            Request::Recover { apply },
            Request::Undo { apply },
            Request::RestoreBaseline(RestoreBaselineRequest {
                scope: RestoreScope::All,
                on_conflict: ConflictPolicy::Report,
                apply,
            }),
        ] {
            let (request, _) = with_countdown(request, apply, 60);
            let carried = match request {
                Request::Revert { apply, .. }
                | Request::Recover { apply }
                | Request::Undo { apply } => apply,
                Request::RestoreBaseline(restore) => restore.apply,
                other => panic!("{other:?}"),
            };
            assert_eq!(carried.countdown_seconds, 60);
        }
        // Only 20 and 60 are ever sent.
        let (_, options) = with_countdown(Request::Undo { apply }, apply, 45);
        assert_eq!(options.countdown_seconds, 20);
        let (request, options) = with_countdown(
            Request::SetMachineSettings {
                restore_on_uninstall: false,
            },
            ApplyOptions::default(),
            60,
        );
        assert_eq!(options.countdown_seconds, 60);
        assert_eq!(
            request,
            Request::SetMachineSettings {
                restore_on_uninstall: false
            }
        );
    }
}
