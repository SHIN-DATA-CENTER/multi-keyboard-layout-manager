//! The dialogs of a running helper session (design m3 B.6, B.7, B.17): progress, the
//! keep-or-revert countdown and the reconnect wait, from the relay's `SessionView`.
//!
//! The countdown is read aloud once when it opens — what changed, how long until it reverts, how
//! to keep it — and that sentence is also the key test's accessible description, where the focus
//! lands (design m3 E.3, review U10). The seconds left are announced politely at 10 and 5 only,
//! never every second. The helper counts the time; the GUI only shows its `CountdownTick`s.

use mklm_client::session::{Prompt, SessionView};
use mklm_core::OpState;

use super::{SnapshotText, Tone};
use crate::i18n::{self, Lang, ProgressStep};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    pub title: String,
    pub step: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Countdown {
    pub title: String,
    /// What changed, the time limit and how to keep it — also the key test's accessible
    /// description and the one assertive announcement when the dialog opens (design m3 E.3).
    pub message: String,
    pub remaining: u32,
    pub total: u32,
    /// What Windows reports for the keyboard, in words (review U5: no "Raw Input", no hex).
    pub recognition: String,
    pub recognition_tone: Tone,
    /// A polite announcement at 10 and 5 seconds left; empty otherwise (design m3 E.3).
    pub reminder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconnect {
    pub title: String,
    pub instructions: String,
    pub status: String,
    pub status_tone: Tone,
    pub can_keep: bool,
}

/// Where the session is, as the UI thread knows it (`state::SessionPhase`, plus what the user
/// answered). The dialog says what MKLM waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    /// The helper is being launched: the UAC prompt may be up.
    #[default]
    Launching,
    /// The helper is connected and works.
    Running,
    /// The user answered the question: keep (true) or revert (false).
    Answered { keep: bool },
    /// "Decide later", or quitting while a reconnect waits.
    Leaving,
    /// The helper stopped; MKLM reads the journal.
    HelperLost,
    /// The recovery after a lost helper was agreed to.
    Recovering,
}

/// The progress dialog while the helper works (before and after any question).
pub fn progress(stage: Stage, view: &SessionView, lang: Lang) -> Progress {
    let step = match stage {
        // A recovery session waits for its own UAC prompt first; say what it is for.
        Stage::Launching => i18n::progress_step(ProgressStep::WaitingForPrompt, lang),
        Stage::Answered { keep: true } => i18n::progress_step(ProgressStep::Keeping, lang),
        Stage::Answered { keep: false } => i18n::progress_step(ProgressStep::Reverting, lang),
        Stage::Leaving => i18n::progress_step(ProgressStep::Leaving, lang),
        Stage::HelperLost => i18n::progress_step(ProgressStep::HelperLost, lang),
        Stage::Recovering | Stage::Running => {
            if let Some(id) = &view.resetting {
                i18n::progress_resetting(&view.display_name(id), lang)
            } else if view.state == Some(OpState::RevertPending) {
                i18n::state(OpState::RevertPending, lang)
            } else if let Some((step, of)) = view.steps_written {
                i18n::progress_step(ProgressStep::Written(step, of), lang)
            } else if view.planned {
                i18n::progress_step(ProgressStep::Journaled, lang)
            } else if stage == Stage::Recovering {
                i18n::progress_step(ProgressStep::Recovering, lang)
            } else {
                i18n::progress_step(ProgressStep::Preparing, lang)
            }
        }
    };
    Progress {
        title: i18n::progress_title(lang),
        step,
        detail: String::new(),
    }
}

/// The countdown dialog; `None` when no countdown is open.
pub fn countdown(view: &SessionView, lang: Lang) -> Option<Countdown> {
    let Prompt::Countdown {
        seconds,
        remaining,
        verified,
        ..
    } = &view.prompt
    else {
        return None;
    };
    let changed: Vec<String> = view
        .keyboards
        .iter()
        .filter(|kb| kb.changes)
        .map(|kb| {
            let layout = kb
                .layout_after
                .as_ref()
                .map_or_else(|| "?".to_string(), |t| i18n::table(t, lang));
            i18n::countdown_switched(&kb.display_name, &layout, lang)
        })
        .collect();
    let arrivals: Vec<String> = view
        .arrivals
        .iter()
        .map(|a| {
            i18n::arrival(
                &view.display_name(&a.instance_id),
                &i18n::recognized(a.reported, lang),
                lang,
            )
        })
        .collect();
    let reminder = match *remaining {
        10 | 5 => i18n::countdown_reminder(*remaining, lang),
        _ => String::new(),
    };
    // The time limit and the way to keep it are part of the first sentence a screen reader
    // hears (review U10): the focus lands on the key test, whose description this is.
    Some(Countdown {
        title: i18n::countdown_title(lang),
        // Japanese sentences follow each other without a space.
        message: match lang {
            Lang::Ja => format!(
                "{}{}",
                changed.concat(),
                i18n::countdown_how(*seconds, lang)
            ),
            Lang::En => format!(
                "{} {}",
                changed.join(" "),
                i18n::countdown_how(*seconds, lang)
            ),
        },
        remaining: *remaining,
        total: *seconds,
        recognition: i18n::recognition(*verified, &arrivals, lang),
        recognition_tone: if *verified {
            Tone::Success
        } else {
            Tone::Warning
        },
        reminder,
    })
}

/// The names of the keyboards a reconnect waits for.
pub fn reconnect_names(view: &SessionView) -> Vec<String> {
    let Prompt::Reconnect { instance_ids, .. } = &view.prompt else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for name in instance_ids.iter().map(|id| view.display_name(id)) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The reconnect dialog; `None` when no reconnect question is open.
pub fn reconnect(view: &SessionView, lang: Lang) -> Option<Reconnect> {
    if !matches!(view.prompt, Prompt::Reconnect { .. }) {
        return None;
    }
    let arrived = view.all_arrivals_verified();
    Some(Reconnect {
        title: i18n::reconnect_title(lang),
        instructions: i18n::reconnect_instructions(&reconnect_names(view), lang),
        status: i18n::reconnect_status(arrived, lang),
        status_tone: if arrived { Tone::Success } else { Tone::Info },
        // Keeping before the keyboard reports the new type is allowed (design m2 C.11): the
        // result then says it takes effect at the reconnect.
        can_keep: true,
    })
}

impl SnapshotText for Progress {
    fn snapshot_text(&self) -> String {
        format!(
            "title: {}\nstep: {}\ndetail: {}\n",
            self.title, self.step, self.detail
        )
    }
}

impl SnapshotText for Countdown {
    fn snapshot_text(&self) -> String {
        format!(
            "title: {}\nmessage: {}\nremaining: {} / {}\nrecognition: {}\nrecognition tone: {:?}\nreminder: {}\n",
            self.title,
            self.message,
            self.remaining,
            self.total,
            self.recognition,
            self.recognition_tone,
            self.reminder
        )
    }
}

impl SnapshotText for Reconnect {
    fn snapshot_text(&self) -> String {
        format!(
            "title: {}\ninstructions: {}\nstatus: {}\nstatus tone: {:?}\ncan keep: {}\n",
            self.title, self.instructions, self.status, self.status_tone, self.can_keep
        )
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::session::Arrival;
    use mklm_core::{Event, ExpectedKeyboard, KeyboardType, LayoutTable, OpId, PendingAction};

    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    fn planned(view: &mut SessionView) {
        view.observe(&Event::Planned {
            op_id: op(),
            steps: Vec::new(),
            apply: Some(PendingAction::ResetKeyboard),
            keyboards: vec![ExpectedKeyboard {
                instance_id: KEYCHRON.into(),
                display_name: "Keychron Receiver".into(),
                expected_type: Some(KeyboardType::JIS),
                layout_after: Some(LayoutTable::Jis),
                changes: true,
            }],
        });
    }

    fn view() -> SessionView {
        let mut view = SessionView::default();
        planned(&mut view);
        view.arrivals.push(Arrival {
            instance_id: KEYCHRON.into(),
            reported: Some(KeyboardType::JIS),
            expected: KeyboardType::JIS,
        });
        view.observe(&Event::CountdownStarted {
            op_id: op(),
            seconds: 20,
            verified: true,
        });
        view
    }

    /// Plain Japanese only (review U5): device names and allowed words.
    fn plain_japanese(snapshot: &str) {
        let values = crate::vm::snapshot_values(snapshot);
        let unexpected = crate::vm::unexpected_latin(&values, &["Keychron Receiver"]);
        assert!(unexpected.is_empty(), "{unexpected:?} in {values}");
    }

    fn tick(view: &mut SessionView, remaining: u32) {
        view.observe(&Event::CountdownTick {
            op_id: op(),
            remaining,
        });
    }

    #[test]
    fn countdown_texts() {
        let ja = countdown(&view(), Lang::Ja).unwrap();
        assert_eq!(ja.remaining, 20);
        assert_eq!(
            ja.message,
            "Keychron Receiver を JIS に切り替えました。あと 20 秒で自動的に元に戻ります。\
             そのキーボードで Shift+2 を押して確かめてから（\" なら JIS、@ なら US）、Tab で\
             ［このままにする］へ移って押してください。"
        );
        assert_eq!(
            ja.recognition,
            "Windows の認識: Keychron Receiver は JIS 配列です ✓"
        );
        assert_eq!(ja.recognition_tone, Tone::Success);
        assert_eq!(ja.reminder, "");
        // Plain Japanese only (review U5): device names and allowed words.
        for text in [&ja.message, &ja.recognition] {
            assert!(
                crate::vm::unexpected_latin(text, &["Keychron Receiver"]).is_empty(),
                "{text}"
            );
        }
        let mut ten = view();
        tick(&mut ten, 10);
        assert_eq!(
            countdown(&ten, Lang::Ja).unwrap().reminder,
            "あと 10 秒で元に戻ります"
        );
        assert!(reconnect(&view(), Lang::Ja).is_none());
        assert_eq!(
            progress(Stage::Launching, &SessionView::default(), Lang::En).step,
            "Waiting for the administrator prompt…"
        );
    }

    #[test]
    fn the_seconds_are_announced_at_ten_and_five_only() {
        let mut view = view();
        for remaining in (0..20).rev() {
            tick(&mut view, remaining);
            let reminder = countdown(&view, Lang::En).unwrap().reminder;
            match remaining {
                10 => assert_eq!(reminder, "Reverting in 10 seconds"),
                5 => assert_eq!(reminder, "Reverting in 5 seconds"),
                _ => assert_eq!(reminder, "", "{remaining}"),
            }
        }
    }

    #[test]
    fn countdown_snapshots() {
        let mut unverified = SessionView::default();
        planned(&mut unverified);
        unverified.observe(&Event::KeyboardArrived {
            instance_id: KEYCHRON.into(),
            reported: Some(KeyboardType { ty: 8, subtype: 2 }),
            expected: KeyboardType::JIS,
        });
        unverified.observe(&Event::CountdownStarted {
            op_id: op(),
            seconds: 60,
            verified: false,
        });
        tick(&mut unverified, 5);
        let ja = countdown(&unverified, Lang::Ja).unwrap();
        assert_eq!(
            ja.snapshot_text(),
            "title: 新しい配列を試してください\n\
             message: Keychron Receiver を JIS に切り替えました。あと 60 秒で自動的に元に戻ります。\
             そのキーボードで Shift+2 を押して確かめてから（\" なら JIS、@ なら US）、Tab で\
             ［このままにする］へ移って押してください。\n\
             remaining: 5 / 60\n\
             recognition: Windows の認識をまだ確かめられません。打鍵テストで確かめてください。\n\
             recognition tone: Warning\n\
             reminder: あと 5 秒で元に戻ります\n"
        );
        plain_japanese(&ja.snapshot_text());
        assert_eq!(
            countdown(&view(), Lang::En).unwrap().snapshot_text(),
            "title: Try the new layout\n\
             message: Keychron Receiver switched to JIS. It reverts automatically in 20 seconds. \
             Press Shift+2 on that keyboard to check (\" means JIS, @ means US), then Tab to \
             \"Keep this layout\" and press it.\n\
             remaining: 20 / 20\n\
             recognition: Windows reports: Keychron Receiver: JIS ✓\n\
             recognition tone: Success\n\
             reminder: \n"
        );
        // An unknown type is named without hex (review U5).
        assert_eq!(
            i18n::recognized(Some(KeyboardType { ty: 8, subtype: 2 }), Lang::Ja),
            "不明な種類（8/2）"
        );
    }

    #[test]
    fn reconnect_snapshots() {
        let mut view = SessionView::default();
        planned(&mut view);
        view.observe(&Event::WaitingForReconnect {
            op_id: op(),
            instance_ids: vec![KEYCHRON.into()],
        });
        let ja = reconnect(&view, Lang::Ja).unwrap();
        assert_eq!(
            ja.snapshot_text(),
            "title: キーボードを接続し直してください\n\
             instructions: Keychron Receiver を抜いて差し直してください（Bluetooth は電源をオフにしてからオンにします）。その後 Shift+2 で確かめてください。\n\
             status: 接続し直すのを待っています…\n\
             status tone: Info\n\
             can keep: true\n"
        );
        plain_japanese(&ja.snapshot_text());
        view.observe(&Event::KeyboardArrived {
            instance_id: KEYCHRON.into(),
            reported: Some(KeyboardType::JIS),
            expected: KeyboardType::JIS,
        });
        assert_eq!(
            reconnect(&view, Lang::En).unwrap().snapshot_text(),
            "title: Reconnect the keyboard\n\
             instructions: Unplug and replug Keychron Receiver (Bluetooth: turn it off and on), then try Shift+2.\n\
             status: The keyboard is back and reports the new type ✓\n\
             status tone: Success\n\
             can keep: true\n"
        );
        assert!(countdown(&view, Lang::Ja).is_none());
    }

    #[test]
    fn progress_steps() {
        let step = |stage, view: &SessionView, lang| progress(stage, view, lang).step;
        let empty = SessionView::default();
        assert_eq!(
            step(Stage::Launching, &empty, Lang::Ja),
            "管理者の確認を待っています…"
        );
        assert_eq!(step(Stage::Running, &empty, Lang::Ja), "準備しています…");
        let mut view = SessionView::default();
        planned(&mut view);
        assert_eq!(step(Stage::Running, &view, Lang::Ja), "操作を記録しました");
        view.observe(&Event::StepWritten {
            op_id: op(),
            step: 1,
            of: 1,
        });
        assert_eq!(step(Stage::Running, &view, Lang::En), "Written 1 of 1");
        view.observe(&Event::ResettingKeyboard {
            instance_id: KEYCHRON.into(),
        });
        assert_eq!(
            step(Stage::Running, &view, Lang::Ja),
            "Keychron Receiver をリセットしています…"
        );
        assert_eq!(
            step(Stage::Answered { keep: true }, &view, Lang::Ja),
            "このままにします…"
        );
        assert_eq!(
            step(Stage::Answered { keep: false }, &view, Lang::Ja),
            "元に戻しています…"
        );
        assert_eq!(
            step(Stage::Recovering, &empty, Lang::Ja),
            "キーボードを元に戻しています…"
        );
        let ja = progress(Stage::HelperLost, &empty, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "title: キーボードの設定を変更しています\n\
             step: MKLM の管理用プログラム（mklm-helper.exe）が止まりました。確かめています…\n\
             detail: \n"
        );
        plain_japanese(&ja.snapshot_text());
        assert_eq!(
            progress(Stage::Leaving, &empty, Lang::En).snapshot_text(),
            "title: Changing the keyboard settings\n\
             step: Leaving it for later; the change keeps waiting for you…\n\
             detail: \n"
        );
    }
}
