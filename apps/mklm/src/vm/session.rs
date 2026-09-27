//! The dialogs of a running helper session (design m3 B.6, B.7, B.17): progress, the
//! keep-or-revert countdown and the reconnect wait, from the relay's `SessionView`.

use mklm_client::session::{Prompt, SessionView};
use mklm_core::{KeyboardType, OpState};

use super::Tone;
use crate::i18n::{self, Lang};

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

/// The progress dialog while the helper works (before any question).
pub fn progress(view: &SessionView, lang: Lang) -> Progress {
    let title = match lang {
        Lang::Ja => "キーボードの設定を変更しています",
        Lang::En => "Changing the keyboard settings",
    }
    .to_string();
    let step = if let Some(id) = &view.resetting {
        match lang {
            Lang::Ja => format!("{} をリセットしています…", view.display_name(id)),
            Lang::En => format!("Resetting {}…", view.display_name(id)),
        }
    } else if view.state == Some(OpState::RevertPending) {
        i18n::state(OpState::RevertPending, lang)
    } else if let Some((step, of)) = view.steps_written {
        match lang {
            Lang::Ja => format!("書き込み {step} / {of}"),
            Lang::En => format!("Written {step} of {of}"),
        }
    } else if view.planned {
        match lang {
            Lang::Ja => "操作を記録しました".to_string(),
            Lang::En => "The operation is journaled".to_string(),
        }
    } else {
        match lang {
            Lang::Ja => "管理者の確認を待っています…".to_string(),
            Lang::En => "Waiting for the administrator prompt…".to_string(),
        }
    };
    Progress {
        title,
        step,
        detail: String::new(),
    }
}

/// A reported keyboard type in words: "JIS 配列" / "US 配列" / "不明な種類（hex）".
fn recognized(reported: Option<KeyboardType>, lang: Lang) -> String {
    match (reported, lang) {
        (Some(KeyboardType::JIS), Lang::Ja) => "JIS 配列".into(),
        (Some(KeyboardType::JIS), Lang::En) => "JIS".into(),
        (Some(KeyboardType::US), Lang::Ja) => "US 配列".into(),
        (Some(KeyboardType::US), Lang::En) => "US".into(),
        (Some(other), Lang::Ja) => format!("不明な種類（{other}）"),
        (Some(other), Lang::En) => format!("an unknown type ({other})"),
        (None, Lang::Ja) => "不明".into(),
        (None, Lang::En) => "unknown".into(),
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
            match lang {
                Lang::Ja => format!("{} を {layout} に切り替えました。", kb.display_name),
                Lang::En => format!("{} switched to {layout}.", kb.display_name),
            }
        })
        .collect();
    // The time limit and the way to keep it are part of the first sentence a screen reader
    // hears (review U10): the focus lands on the key test, whose description this is.
    let how = match lang {
        Lang::Ja => format!(
            "あと {seconds} 秒で自動的に元に戻ります。そのキーボードで Shift+2 を押して確かめてから（\" なら JIS、@ なら US）、Tab で『このままにする』へ移って押してください。"
        ),
        Lang::En => format!(
            "It reverts automatically in {seconds} seconds. Press Shift+2 on that keyboard to check (\" means JIS, @ means US), then Tab to \"Keep this layout\" and press it."
        ),
    };
    let arrivals: Vec<String> = view
        .arrivals
        .iter()
        .map(|a| {
            let name = view.display_name(&a.instance_id);
            let kind = recognized(a.reported, lang);
            match lang {
                Lang::Ja => format!("{name} は {kind}です"),
                Lang::En => format!("{name} reports {kind}"),
            }
        })
        .collect();
    let recognition = match (verified, lang) {
        (true, Lang::Ja) => format!("Windows の認識: {} ✓", arrivals.join("、")),
        (true, Lang::En) => format!("Windows reports: {} ✓", arrivals.join(", ")),
        (false, Lang::Ja) => {
            "Windows の認識をまだ確かめられません。打鍵テストで確かめてください".to_string()
        }
        (false, Lang::En) => {
            "Windows has not confirmed it yet; check with the key test".to_string()
        }
    };
    let reminder = match (*remaining, lang) {
        (10 | 5, Lang::Ja) => format!("あと {remaining} 秒で元に戻ります"),
        (10 | 5, Lang::En) => format!("{remaining} seconds left"),
        _ => String::new(),
    };
    Some(Countdown {
        title: match lang {
            Lang::Ja => "新しい配列を試してください".into(),
            Lang::En => "Try the new layout".into(),
        },
        message: format!("{} {how}", changed.join(" ")),
        remaining: *remaining,
        total: *seconds,
        recognition,
        recognition_tone: if *verified {
            Tone::Success
        } else {
            Tone::Warning
        },
        reminder,
    })
}

/// The reconnect dialog; `None` when no reconnect question is open.
pub fn reconnect(view: &SessionView, lang: Lang) -> Option<Reconnect> {
    let Prompt::Reconnect { instance_ids, .. } = &view.prompt else {
        return None;
    };
    let names: Vec<String> = instance_ids
        .iter()
        .map(|id| view.display_name(id))
        .collect();
    let arrived = view.all_arrivals_verified();
    Some(Reconnect {
        title: match lang {
            Lang::Ja => "キーボードを接続し直してください".into(),
            Lang::En => "Reconnect the keyboard".into(),
        },
        instructions: match lang {
            Lang::Ja => format!(
                "{} を抜いて差し直してください（Bluetooth は電源をオフにしてからオンにします）。その後 Shift+2 で確かめてください。",
                names.join("、")
            ),
            Lang::En => format!(
                "Unplug and replug {} (Bluetooth: turn it off and on), then try Shift+2.",
                names.join(", ")
            ),
        },
        status: match (arrived, lang) {
            (true, Lang::Ja) => "接続し直したキーボードが新しい種類を報告しています ✓".into(),
            (true, Lang::En) => "The keyboard is back and reports the new type ✓".into(),
            (false, Lang::Ja) => "接続し直すのを待っています…".into(),
            (false, Lang::En) => "Waiting for the keyboard to reconnect…".into(),
        },
        status_tone: if arrived { Tone::Success } else { Tone::Info },
        can_keep: true,
    })
}

#[cfg(test)]
mod tests {
    use mklm_client::session::Arrival;
    use mklm_core::{Event, ExpectedKeyboard, LayoutTable, OpId, PendingAction};

    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    fn view() -> SessionView {
        let op_id = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        let mut view = SessionView::default();
        view.observe(&Event::Planned {
            op_id: op_id.clone(),
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
        view.arrivals.push(Arrival {
            instance_id: KEYCHRON.into(),
            reported: Some(KeyboardType::JIS),
            expected: KeyboardType::JIS,
        });
        view.observe(&Event::CountdownStarted {
            op_id,
            seconds: 20,
            verified: true,
        });
        view
    }

    #[test]
    fn countdown_texts() {
        let ja = countdown(&view(), Lang::Ja).unwrap();
        assert_eq!(ja.remaining, 20);
        assert_eq!(
            ja.message,
            "Keychron Receiver を JIS に切り替えました。 あと 20 秒で自動的に元に戻ります。\
             そのキーボードで Shift+2 を押して確かめてから（\" なら JIS、@ なら US）、Tab で\
             『このままにする』へ移って押してください。"
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
        ten.observe(&Event::CountdownTick {
            op_id: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
            remaining: 10,
        });
        assert_eq!(
            countdown(&ten, Lang::Ja).unwrap().reminder,
            "あと 10 秒で元に戻ります"
        );
        assert!(reconnect(&view(), Lang::Ja).is_none());
        assert_eq!(
            progress(&SessionView::default(), Lang::En).step,
            "Waiting for the administrator prompt…"
        );
    }
}
