//! Pure presentation and checked requests for the PC's standard-layout change.
use mklm_client::preview::{StandardRole, StandardRow, standard_rows};
use mklm_core::{ApplyOptions, GlobalMode, Layout, SystemSnapshot};
use mklm_ipc::{Request, SetStandardRequest};

use super::change::{ApplyMethod, ChoiceRow};
use crate::i18n::{self, Lang, pick};
use crate::state::{AppState, ChangeDraft, PrepareFailure};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Entry {
    pub visible: bool,
    pub enabled: bool,
    pub note: String,
}

pub fn entry(state: &AppState, lang: Lang) -> Entry {
    let Some(read) = &state.read else {
        return Entry::default();
    };
    let Some(snapshot) = &read.snapshot else {
        return Entry::default();
    };
    if mklm_core::assess(snapshot).mode != GlobalMode::PerKeyboard {
        return Entry {
            note: i18n::standard::fixed_note(lang),
            ..Entry::default()
        };
    }
    Entry {
        visible: true,
        enabled: crate::state::navigation_enabled(state) && !read.summary.blocks_writes(),
        note: super::status::blocked_reason(&read.summary, lang).unwrap_or_default(),
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub text: String,
    pub keep_label: String,
    pub selectable: bool,
    pub keep: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    pub note: String,
    pub choices: Vec<ChoiceRow>,
    pub choice_label: String,
    pub selected: i32,
    pub rows: Vec<Row>,
    pub error: String,
    pub keep_label: String,
    pub reset_label: String,
    pub reset_note: String,
    pub can_reset: bool,
    pub reset: bool,
    pub ready: bool,
    pub editable: bool,
    pub restart_note: String,
    pub details: String,
    pub uac_line: String,
    pub apply_label: String,
}

pub fn page(
    draft: &ChangeDraft,
    display: Option<&SystemSnapshot>,
    elevated: bool,
    uac_notice_seen: bool,
    lang: Lang,
) -> Page {
    let snapshot = draft.prepared.as_ref().map(|p| &p.snapshot).or(display);
    let current = snapshot.and_then(|s| mklm_core::stored_standard(&s.global));
    let mut page = Page {
        title: i18n::standard::title(lang),
        note: i18n::standard::introduction(snapshot.map(|s| &s.os), lang),
        choice_label: i18n::standard::choice_label(lang),
        selected: match draft.standard {
            Some(Layout::Jis) => 0,
            Some(Layout::Us) => 1,
            None => -1,
        },
        keep_label: pick(lang, "今の配列のままにする", "Keep the current layout"),
        reset_label: pick(lang, "割り当てを今すぐ反映する", "Apply assignments now"),
        apply_label: i18n::apply_button(false, !elevated, lang),
        uac_line: if !elevated && uac_notice_seen {
            i18n::uac_line(lang)
        } else {
            String::new()
        },
        ready: request(draft).is_some(),
        editable: draft.preparing.is_none()
            && draft.failure.is_none()
            && draft.prepared.as_ref().is_some_and(|p| p.blocker.is_none()),
        ..Page::default()
    };
    page.choices = [Layout::Jis, Layout::Us]
        .into_iter()
        .map(|layout| ChoiceRow {
            text: i18n::standard::choice(layout, current == Some(layout), lang),
            enabled: page.editable,
            current: current == Some(layout),
            ..ChoiceRow::default()
        })
        .collect();
    let mut details = Vec::new();
    if let Some(prepared) = &draft.prepared {
        details.extend(prepared.warnings.clone());
    }
    if draft.preparing.is_some() {
        page.error = pick(lang, "確認しています…", "Checking…");
    } else if let Some(failure) = &draft.failure {
        page.error = i18n::prepare_failed(
            matches!(failure, PrepareFailure::Incomplete(_)),
            i18n::PreparePlace::ChangePage,
            lang,
        );
        details.push(failure.diagnostic().into());
    } else if let Some(blocker) = draft.prepared.as_ref().and_then(|p| p.blocker.as_ref()) {
        page.error = i18n::block_reason(blocker, &i18n::standard::entry_label(lang), lang);
    } else if let Some(Err(refused)) = &draft.standard_plan {
        page.error = i18n::operation_refused(refused, lang);
    } else if let (Some(prepared), Some(Ok(plan))) = (&draft.prepared, &draft.standard_plan) {
        if plan.change.is_empty() {
            page.error = pick(
                lang,
                "今の標準配列です。変更はありません。",
                "This is the current standard; nothing to change.",
            );
        } else {
            let rows = standard_rows(&prepared.snapshot, plan);
            let reset_names: Vec<_> = rows
                .iter()
                .filter(|row| row.can_reset)
                .map(|row| row.name.clone())
                .collect();
            page.can_reset = !reset_names.is_empty();
            page.reset = page.can_reset && draft.resolved_method() == ApplyMethod::Live;
            if page.can_reset {
                let names = i18n::name_list(&reset_names, lang);
                page.reset_note = pick(
                    lang,
                    &format!(
                        "{names} を数秒間止めて、今の配列の割り当てを反映します。打つ配列は変わりません。その間は、ほかのキーボードかマウスで操作します。チェックを外すと、割り当ては PC の再起動で反映されます。標準配列の変更には、どちらの場合も再起動が必要です。"
                    ),
                    &format!(
                        "{names} stop for a few seconds to apply their assigned layouts. What they type stays the same. Use another keyboard or the mouse meanwhile. With this unchecked, assignments take effect at the PC restart. The standard-layout change requires a restart either way."
                    ),
                );
            }
            page.rows = rows
                .into_iter()
                .map(|row| row_view(row, plan.change.to, lang))
                .collect();
            page.restart_note = i18n::standard::restart_note(lang);
            if prepared.snapshot.os.remote_session
                || prepared
                    .snapshot
                    .keyboards
                    .iter()
                    .any(|kb| kb.is_remote_desktop())
            {
                page.restart_note.push_str("\n\n");
                page.restart_note
                    .push_str(&i18n::standard::remote_note(lang));
            }
            details.extend(
                super::change::plan_lines(&prepared.snapshot, &plan.plan, lang)
                    .into_iter()
                    .map(|line| {
                        format!(
                            "{}\n  {}: {} → {}",
                            line.key, line.name, line.now, line.after
                        )
                    }),
            );
            details.push(pick(lang,
                "確認: 全体の OverrideKeyboardType/Subtype がないこと（キーボードごとモードを維持）。\nINV-PS2: 各段階で成立。",
                "Check: no global OverrideKeyboardType/Subtype (per-keyboard mode is retained).\nINV-PS2: holds after every step."));
        }
    } else if draft.standard.is_none() {
        page.error = i18n::standard::unknown_standard(lang);
    }
    if current.is_none()
        && let Some(snapshot) = snapshot
    {
        details.push(format!(
            "LayerDriver JPN: {:?}\nOverrideKeyboardIdentifier: {:?}",
            snapshot.global.layer_driver_jpn, snapshot.global.override_keyboard_identifier
        ));
    }
    page.details = details.join("\n");
    page
}

fn row_view(row: StandardRow, to: Layout, lang: Lang) -> Row {
    let before = row
        .before
        .as_ref()
        .map_or_else(|| "—".into(), |layout| i18n::effective(layout, lang));
    let after = row.after.as_ref().map_or_else(
        || pick(lang, "配列が混在", "Mixed layouts"),
        |table| i18n::table(table, lang),
    );
    let target = match to {
        Layout::Jis => "JIS",
        Layout::Us => "US",
    };
    let text = match row.role {
        StandardRole::Assigned { .. }
        | StandardRole::NotAssignable {
            follows: Some(false),
        } => pick(
            lang,
            &format!("{after}（変わりません）"),
            &format!("{after} (unchanged)"),
        ),
        StandardRole::Pinned => pick(
            lang,
            &format!("{after}（変わりません。今の配列を割り当てます）"),
            &format!("{after} (unchanged: its current layout is assigned)"),
        ),
        StandardRole::Mixed => pick(
            lang,
            &format!("{after}（一部だけ標準に従っています。その部分に今の配列を割り当てます）"),
            &format!(
                "{after} (only some collections follow the standard; their current layout is assigned)"
            ),
        ),
        StandardRole::Follows => {
            let mut text = pick(
                lang,
                &format!("{after} に変わります（新しい標準に従う）"),
                &format!("Becomes {after} (follows the new standard)"),
            );
            if to == Layout::Jis {
                text.push_str(&pick(
                    lang,
                    "。打鍵での確認がまだです。後で Shift+2 で確かめてください。",
                    ". Not verified by typing yet; check with Shift+2 afterwards.",
                ));
            }
            text
        }
        StandardRole::RemoteDesktop => pick(
            lang,
            &format!("{target} になることがあります（下の説明）"),
            &format!("May become {target} (see below)"),
        ),
        StandardRole::NotAssignable {
            follows: Some(true),
        } => pick(
            lang,
            &format!("{target} になります（標準に従います。MKLM は配列を割り当てられません）"),
            &format!("Becomes {target} (follows the standard; MKLM cannot assign a layout)"),
        ),
        StandardRole::NotAssignable { follows: None } => pick(
            lang,
            &format!("標準に従う場合は {target} になります（MKLM は配列を割り当てられません）"),
            &format!("Becomes {target} if it follows the standard (MKLM cannot assign a layout)"),
        ),
    };
    let name = format!(
        "{}{}",
        row.name,
        if row.present {
            String::new()
        } else {
            pick(lang, "（未接続）", " (not connected)")
        }
    );
    Row {
        id: row.members.first().cloned().unwrap_or_default(),
        keep_label: pick(
            lang,
            &format!("{}を今の配列のままにする", row.name),
            &format!("Keep {} at its current layout", row.name),
        ),
        name,
        text: pick(
            lang,
            &format!("設定した配列（今）: {before} → 再起動の後: {text}"),
            &format!("Set now: {before} → After the restart: {text}"),
        ),
        selectable: matches!(
            row.role,
            StandardRole::Pinned | StandardRole::Follows | StandardRole::Mixed
        ),
        keep: !matches!(row.role, StandardRole::Follows),
    }
}

/// Only keyboards actually pinned and eligible for a reset inform the default method.
pub fn reset_targets(draft: &ChangeDraft) -> Vec<String> {
    let (Some(prepared), Some(Ok(plan))) = (&draft.prepared, &draft.standard_plan) else {
        return Vec::new();
    };
    if plan.change.is_empty() {
        return Vec::new();
    }
    standard_rows(&prepared.snapshot, plan)
        .into_iter()
        .filter(|row| row.can_reset)
        .flat_map(|row| row.members)
        .collect()
}

pub fn request(draft: &ChangeDraft) -> Option<(Request, ApplyOptions)> {
    if !draft.standard_change
        || draft.preparing.is_some()
        || draft.failure.is_some()
        || draft.prepared.as_ref()?.blocker.is_some()
    {
        return None;
    }
    let plan = draft.standard_plan.as_ref()?.as_ref().ok()?;
    if plan.change.is_empty() {
        return None;
    }
    let apply = if reset_targets(draft).is_empty() {
        ApplyMethod::Restart
    } else {
        draft.resolved_method()
    }
    .options();
    Some((
        Request::SetStandard(SetStandardRequest {
            standard: plan.change.to,
            follow: draft.follow.clone(),
            apply,
            expected: Some(mklm_client::preview::expected(&plan.plan)),
        }),
        apply,
    ))
}
