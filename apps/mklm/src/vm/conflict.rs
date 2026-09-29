//! Conflict resolution (design m2 D.8; m3 B.10): an operation in `Conflict`, shown per keyboard
//! (the records of one target: a device's Type/Subtype pair, or the PC-wide values) in layout
//! names, with a recommendation (review U6). The per-value numbers and choices are folded into
//! "詳細" for whoever needs them.
//!
//! Terms, the same everywhere: 操作の前の値 (`before`), 操作で書こうとした値 (`intended`), MKLM
//! 導入前の値 (`baseline`), 今の値 (`current`).
//!
//! Both values of a pair always get the same choice unless the details override one of them, so
//! a pair MKLM never writes (4/2, 8/0) cannot be made by accident. A choice that would break
//! INV-PS2 is refused by the helper (`PlanRejected`); the page then says which PS/2 keyboards
//! would be left without a fixed layout and the two ways out ([`inv_ps2_error`]).

use mklm_core::{
    ConflictInfo, ConflictPolicy, JournalEntry, KeyboardType, LayoutTable, RegValue,
    ResolutionChoice, RestoreScope, ValueChoice, ValueKey, ValueRecord, WriteTarget, value_eq,
    value_names,
};

use super::SnapshotText;
use super::journal::{NameOf, kind_text};
use crate::i18n::{Lang, journal_pages as text};

/// One way to resolve a keyboard, by layout name, duplicates removed ("変更前の US に戻す",
/// "MKLM が設定した JIS にする", "今の値のまま"; "導入前" only when it differs from "変更前").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictOption {
    pub label: String,
    pub choice: ResolutionChoice,
}

/// One value, for the folded details.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictValue {
    /// The record's index in the entry.
    pub record: usize,
    pub key: String,
    pub name: String,
    pub current: String,
    pub last_written: String,
    pub before: String,
    pub intended: String,
    pub baseline: String,
    pub write_error: String,
    /// A per-value choice made in the details; `None` follows the keyboard's option.
    pub choice: Option<ResolutionChoice>,
    /// Every number above in one line (`i18n::journal_pages::conflict_value_line`).
    pub line: String,
}

/// One keyboard (or the PC-wide values) of the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictKeyboard {
    /// What the row stands for: the keyboard's instance ID, or empty for the PC-wide values. The
    /// row's key in the displayed list (design m3 A.6): names can repeat, targets do not.
    pub target: String,
    /// "Keychron Receiver", or "PC 全体の設定".
    pub name: String,
    /// "今の値: 不明な種類（8/2）— MKLM 以外が変更".
    pub now: String,
    pub options: Vec<ConflictOption>,
    /// The option chosen (starts at the recommendation).
    pub selected: usize,
    /// The recommended option, marked "（おすすめ）".
    pub recommended: Option<usize>,
    pub values: Vec<ConflictValue>,
    /// A value MKLM could not write (design m2 C3): what to do; the error is in the details.
    pub write_error: String,
    /// A restore to before MKLM: what the keyboard had then ("MKLM 導入前: US"); empty for an
    /// operation in conflict.
    pub baseline: String,
}

/// The conflict page: an operation in `Conflict` (per keyboard, [`conflict_keyboards`]), or a
/// restore to before MKLM that stopped before writing ([`restore_conflict_page`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictPage {
    /// The entry in conflict (full operation ID); empty when there is none.
    pub op_id: String,
    pub operation: String,
    pub keyboards: Vec<ConflictKeyboard>,
    /// The helper refused the last choices (INV-PS2), or another problem to show first.
    pub error: String,
    pub uac_line: String,
    /// No entry in conflict: "MKLM 以外による変更は見つかっていません。".
    pub empty_note: String,
    /// A restore that stopped: what happened (nothing was written).
    pub restore_note: String,
    /// A restore that stopped: the two ways to send it again, in [`RESTORE_POLICIES`] order;
    /// empty for an operation in conflict (then each keyboard has its options).
    pub restore_choices: Vec<String>,
    /// The index chosen in `restore_choices` (starts at the recommendation).
    pub restore_selected: usize,
}

impl ConflictPage {
    /// Something can be sent ("選んだとおりにする").
    pub fn can_resolve(&self) -> bool {
        !self.keyboards.is_empty() && (!self.op_id.is_empty() || !self.restore_choices.is_empty())
    }
}

/// The ways to send a stopped restore again, in the order of `ConflictPage::restore_choices`.
pub const RESTORE_POLICIES: [ConflictPolicy; 2] = [ConflictPolicy::Skip, ConflictPolicy::Overwrite];

/// The recommendation for a device whose Type/Subtype pair is `current` now (review U6): a
/// pair MKLM would never write (outside {4/0, 7/2}) is most likely a mistake — go back to the
/// value before the operation; a valid pair (e.g. set in the Settings app) is kept.
pub fn recommend(current: Option<KeyboardType>) -> ResolutionChoice {
    match current {
        Some(KeyboardType::US | KeyboardType::JIS) => ResolutionChoice::KeepCurrent,
        _ => ResolutionChoice::UseBefore,
    }
}

/// The per-value override choices in the details, in the order of
/// `i18n::journal_pages::conflict_value_options` (index 0 follows the keyboard's option).
pub const VALUE_CHOICES: [Option<ResolutionChoice>; 5] = [
    None,
    Some(ResolutionChoice::KeepCurrent),
    Some(ResolutionChoice::UseBefore),
    Some(ResolutionChoice::UseIntended),
    Some(ResolutionChoice::UseBaseline),
];

/// The index of `choice` in [`VALUE_CHOICES`].
pub fn value_choice_index(choice: Option<ResolutionChoice>) -> usize {
    VALUE_CHOICES
        .iter()
        .position(|candidate| *candidate == choice)
        .unwrap_or(0)
}

/// Which value names form the type pair of a target.
fn pair_names(
    target: &WriteTarget,
    records: &[&ValueRecord],
) -> Option<(&'static str, &'static str)> {
    let has = |name: &str| records.iter().any(|r| r.name.eq_ignore_ascii_case(name));
    match target {
        WriteTarget::Device { .. }
            if has(value_names::HID_TYPE) || has(value_names::HID_SUBTYPE) =>
        {
            Some((value_names::HID_TYPE, value_names::HID_SUBTYPE))
        }
        _ if has(value_names::PS2_TYPE) || has(value_names::PS2_SUBTYPE) => {
            Some((value_names::PS2_TYPE, value_names::PS2_SUBTYPE))
        }
        _ => None,
    }
}

/// The pair `values` (one per record of the group) make, in words.
fn pair(
    target: &WriteTarget,
    records: &[&ValueRecord],
    values: &[Option<RegValue>],
) -> text::PairName {
    let value_of = |name: &str| {
        records
            .iter()
            .zip(values)
            .find(|(record, _)| record.name.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value.clone())
    };
    let Some((type_name, subtype_name)) = pair_names(target, records) else {
        return match value_of(value_names::LAYER_DRIVER_JPN) {
            Some(RegValue::Sz { value }) => {
                text::PairName::Standard(LayoutTable::from_layer_driver(Some(&value)))
            }
            Some(RegValue::Absent) => {
                text::PairName::Standard(LayoutTable::from_layer_driver(None))
            }
            _ => text::PairName::Unknown,
        };
    };
    let global = matches!(target, WriteTarget::Global);
    let hid = type_name == value_names::HID_TYPE;
    match (value_of(type_name), value_of(subtype_name)) {
        (Some(RegValue::Dword { value: ty }), Some(RegValue::Dword { value: subtype })) => {
            let pair = KeyboardType::new(ty, subtype);
            if global {
                text::PairName::GlobalFixed(pair)
            } else {
                text::PairName::Type(pair)
            }
        }
        (Some(RegValue::Absent), Some(RegValue::Absent)) => match (global, hid) {
            (true, _) => text::PairName::GlobalAbsent,
            (false, true) => text::PairName::HidAbsent,
            (false, false) => text::PairName::Ps2Absent,
        },
        _ => text::PairName::Unknown,
    }
}

/// The layout `values` give the main target of `entry`, in words ("US", "固定モード（JIS）"): the
/// requested keyboard of a change, the PC-wide values of a migration, else the first target.
/// The recovery and undo previews say where a revert goes with it ("US（変更前）に戻ります").
pub fn main_pair_name(
    entry: &JournalEntry,
    values: &dyn Fn(&ValueRecord) -> Option<RegValue>,
    lang: Lang,
) -> String {
    let main = match &entry.kind {
        mklm_core::OpKind::SetLayout { requested, .. } => entry.records.iter().find(|record| {
            matches!(&record.target, WriteTarget::Device { instance_id } if instance_id.eq_ignore_ascii_case(requested))
        }),
        mklm_core::OpKind::Migrate { .. } => entry
            .records
            .iter()
            .find(|record| matches!(record.target, WriteTarget::Global)),
        // The standard itself, not the check-only records of the fixed-mode pair (design
        // standard-layout B.2).
        mklm_core::OpKind::SetStandard { .. } => entry.records.iter().find(|record| {
            matches!(record.target, WriteTarget::Global) && !record.is_check_only()
        }),
        mklm_core::OpKind::RestoreBaseline { .. } | mklm_core::OpKind::Cleanup { .. } => None,
    }
    .or_else(|| entry.records.first());
    let set_standard = matches!(entry.kind, mklm_core::OpKind::SetStandard { .. });
    let Some(main) = main else {
        return text::pair_name(text::PairName::Unknown, lang);
    };
    let records: Vec<&ValueRecord> = entry
        .records
        .iter()
        .filter(|record| record.target == main.target && !(set_standard && record.is_check_only()))
        .collect();
    let chosen: Vec<Option<RegValue>> = records.iter().map(|record| values(record)).collect();
    text::pair_name(pair(&main.target, &records, &chosen), lang)
}

/// The type pair of a named pair, for the recommendation.
fn pair_type(pair: text::PairName) -> Option<KeyboardType> {
    match pair {
        text::PairName::Type(ty) | text::PairName::GlobalFixed(ty) => Some(ty),
        _ => None,
    }
}

/// The values `choice` writes to the group's records (`None` where it is not known).
fn values_of(
    records: &[&ValueRecord],
    current: &[Option<RegValue>],
    choice: ResolutionChoice,
) -> Vec<Option<RegValue>> {
    records
        .iter()
        .zip(current)
        .map(|(record, current)| match choice {
            ResolutionChoice::KeepCurrent => current.clone(),
            ResolutionChoice::UseBefore => Some(record.before.clone()),
            ResolutionChoice::UseIntended => Some(record.intended.clone()),
            ResolutionChoice::UseBaseline => Some(record.baseline.clone()),
        })
        .collect()
}

fn same_values(records: &[&ValueRecord], a: &[Option<RegValue>], b: &[Option<RegValue>]) -> bool {
    records
        .iter()
        .zip(a.iter().zip(b))
        .all(|(record, pair)| match pair {
            (Some(a), Some(b)) => value_eq(&record.name, a, b),
            _ => false,
        })
}

/// The keyboards of `entry`: records grouped by target, `current` reads the value stored now
/// (the display snapshot; the value seen when the conflict was found when it cannot be read),
/// `name` names a device (the snapshot's display name). Options by layout name with duplicates
/// removed; `selected` = `recommended`.
pub fn conflict_keyboards(
    entry: &JournalEntry,
    current: &dyn Fn(&ValueRecord) -> Option<RegValue>,
    name: &dyn Fn(&str) -> String,
    lang: Lang,
) -> Vec<ConflictKeyboard> {
    let mut groups: Vec<(&WriteTarget, Vec<usize>)> = Vec::new();
    for (index, record) in entry.records.iter().enumerate() {
        let same_target = |target: &WriteTarget| match (target, &record.target) {
            (WriteTarget::Device { instance_id: a }, WriteTarget::Device { instance_id: b }) => {
                a.eq_ignore_ascii_case(b)
            }
            (WriteTarget::Global, WriteTarget::Global) => true,
            _ => false,
        };
        match groups.iter_mut().find(|(target, _)| same_target(target)) {
            Some((_, indices)) => indices.push(index),
            None => groups.push((&record.target, vec![index])),
        }
    }
    let mut keyboards: Vec<ConflictKeyboard> = groups
        .into_iter()
        .map(|(target, indices)| {
            let records: Vec<&ValueRecord> = indices.iter().map(|i| &entry.records[*i]).collect();
            let now: Vec<Option<RegValue>> = records
                .iter()
                .map(|record| current(record).or_else(|| record.conflict.clone()))
                .collect();
            keyboard(target, &indices, &records, &now, name, lang)
        })
        .collect();
    number_same_names(&mut keyboards);
    keyboards
}

fn keyboard(
    target: &WriteTarget,
    indices: &[usize],
    records: &[&ValueRecord],
    now: &[Option<RegValue>],
    name: &dyn Fn(&str) -> String,
    lang: Lang,
) -> ConflictKeyboard {
    let now_pair = pair(target, records, now);
    let known = now.iter().all(Option::is_some);
    let outside = records.iter().zip(now).any(|(record, now)| {
        let expected = record.last_written.as_ref().unwrap_or(&record.intended);
        now.as_ref()
            .is_some_and(|now| !value_eq(&record.name, now, expected))
    });
    // The options, duplicates by the values they write removed (the first one stays).
    let mut options: Vec<(ConflictOption, Vec<Option<RegValue>>)> = Vec::new();
    for choice in [
        ResolutionChoice::UseBefore,
        ResolutionChoice::UseIntended,
        ResolutionChoice::KeepCurrent,
        ResolutionChoice::UseBaseline,
    ] {
        let values = values_of(records, now, choice);
        if options
            .iter()
            .any(|(_, existing)| same_values(records, existing, &values))
        {
            continue;
        }
        let named = text::pair_name(pair(target, records, &values), lang);
        let label = match choice {
            ResolutionChoice::UseBefore => text::ConflictOptionText::Before(&named),
            ResolutionChoice::UseIntended => text::ConflictOptionText::Intended(&named),
            ResolutionChoice::KeepCurrent => text::ConflictOptionText::KeepCurrent,
            ResolutionChoice::UseBaseline => text::ConflictOptionText::Baseline(&named),
        };
        options.push((
            ConflictOption {
                label: text::conflict_option(label, false, lang),
                choice,
            },
            values,
        ));
    }
    // The recommendation, or the option that writes the same values.
    let wanted = recommend(pair_type(now_pair.clone()));
    let wanted_values = values_of(records, now, wanted);
    let recommended = options
        .iter()
        .position(|(option, _)| option.choice == wanted)
        .or_else(|| {
            options
                .iter()
                .position(|(_, values)| same_values(records, values, &wanted_values))
        });
    if let Some(index) = recommended {
        let named = text::pair_name(pair(target, records, &options[index].1), lang);
        let label = match options[index].0.choice {
            ResolutionChoice::UseBefore => text::ConflictOptionText::Before(&named),
            ResolutionChoice::UseIntended => text::ConflictOptionText::Intended(&named),
            ResolutionChoice::KeepCurrent => text::ConflictOptionText::KeepCurrent,
            ResolutionChoice::UseBaseline => text::ConflictOptionText::Baseline(&named),
        };
        options[index].0.label = text::conflict_option(label, true, lang);
    }
    let values = value_lines(records, indices, now, lang);
    let now_text = known.then(|| text::pair_name(now_pair, lang));
    ConflictKeyboard {
        target: target_key(target),
        name: match target {
            WriteTarget::Device { instance_id } => name(instance_id),
            WriteTarget::Global => text::conflict_global_name(lang),
        },
        now: text::conflict_now(now_text.as_deref(), outside, lang),
        selected: recommended.unwrap_or(0),
        recommended,
        options: options.into_iter().map(|(option, _)| option).collect(),
        write_error: if records.iter().any(|record| record.write_error.is_some()) {
            text::conflict_write_error(lang)
        } else {
            String::new()
        },
        values,
        baseline: String::new(),
    }
}

/// The per-value numbers of a group of records (the folded details).
fn value_lines(
    records: &[&ValueRecord],
    indices: &[usize],
    now: &[Option<RegValue>],
    lang: Lang,
) -> Vec<ConflictValue> {
    let shown = |value: &RegValue| crate::i18n::reg_value(value, lang);
    records
        .iter()
        .zip(indices)
        .zip(now)
        .map(|((record, index), now)| {
            let current = now.as_ref().map_or_else(|| "?".to_string(), shown);
            let last_written = record
                .last_written
                .as_ref()
                .map_or_else(|| "—".to_string(), shown);
            let line = text::conflict_value_line(
                &text::ValueNumbers {
                    key: &record.key_path,
                    name: &record.name,
                    current: &current,
                    last_written: &last_written,
                    before: &shown(&record.before),
                    intended: &shown(&record.intended),
                    baseline: &shown(&record.baseline),
                    write_error: record.write_error.as_deref(),
                },
                lang,
            );
            ConflictValue {
                record: *index,
                key: record.key_path.clone(),
                name: record.name.clone(),
                current,
                last_written,
                before: shown(&record.before),
                intended: shown(&record.intended),
                baseline: shown(&record.baseline),
                write_error: record.write_error.clone().unwrap_or_default(),
                choice: None,
                line,
            }
        })
        .collect()
}

/// The target a key path names (`ValueKey::key_path` the other way round).
fn target_of(key_path: &str) -> WriteTarget {
    let global = ValueKey {
        target: WriteTarget::Global,
        name: String::new(),
    }
    .key_path();
    if key_path.eq_ignore_ascii_case(&global) {
        return WriteTarget::Global;
    }
    let instance_id = key_path
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case(r"Enum\"))
        .and_then(|_| key_path.get(5..))
        .and_then(|rest| {
            let suffix = r"\Device Parameters";
            rest.len()
                .checked_sub(suffix.len())
                .filter(|end| {
                    rest.get(*end..)
                        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
                })
                .and_then(|end| rest.get(..end))
        })
        .unwrap_or(key_path);
    WriteTarget::Device {
        instance_id: instance_id.to_string(),
    }
}

/// A reported conflict as a record, so that it is named like the records of an operation.
fn conflict_record(info: &ConflictInfo) -> ValueRecord {
    ValueRecord {
        target: target_of(&info.key_path),
        key_path: info.key_path.clone(),
        name: info.name.clone(),
        baseline: info.baseline.clone(),
        before: info.before.clone(),
        intended: info.intended.clone(),
        last_written: info.last_written.clone(),
        conflict: Some(info.current.clone()),
        resolve_to: None,
        write_error: info.write_error.clone(),
        skipped: None,
    }
}

/// A restore to before MKLM (`scope`) that stopped before writing because the values in
/// `conflicts` were changed outside MKLM (`ConflictPolicy::Report`, design m3 B.10): per keyboard
/// what it has now and had before MKLM, and the two ways to send the restore again — leave those
/// values (`Skip`) or put them back too (`Overwrite`). The recommendation follows [`recommend`]:
/// a pair MKLM would never write is put back, a valid one is left. `chosen` is the user's pick.
pub fn restore_conflict_page(
    scope: &RestoreScope,
    conflicts: &[ConflictInfo],
    chosen: Option<ConflictPolicy>,
    current: &dyn Fn(&ValueRecord) -> Option<RegValue>,
    name_of: NameOf<'_>,
    lang: Lang,
) -> ConflictPage {
    let records: Vec<ValueRecord> = conflicts.iter().map(conflict_record).collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match groups.iter_mut().find(|group| {
            records[group[0]]
                .key_path
                .eq_ignore_ascii_case(&record.key_path)
        }) {
            Some(group) => group.push(index),
            None => groups.push(vec![index]),
        }
    }
    let mut put_back = false;
    let mut keyboards: Vec<ConflictKeyboard> = groups
        .iter()
        .map(|indices| {
            let group: Vec<&ValueRecord> = indices.iter().map(|i| &records[*i]).collect();
            let target = &group[0].target;
            let now: Vec<Option<RegValue>> = group
                .iter()
                .map(|record| current(record).or_else(|| record.conflict.clone()))
                .collect();
            let baseline: Vec<Option<RegValue>> = group
                .iter()
                .map(|record| Some(record.baseline.clone()))
                .collect();
            let now_pair = pair(target, &group, &now);
            put_back |= recommend(pair_type(now_pair.clone())) == ResolutionChoice::UseBefore;
            let numbers: Vec<usize> = indices.iter().map(|i| conflicts[*i].record).collect();
            ConflictKeyboard {
                target: target_key(target),
                name: match target {
                    WriteTarget::Device { instance_id } => name_of(instance_id),
                    WriteTarget::Global => text::conflict_global_name(lang),
                },
                now: text::conflict_now(Some(&text::pair_name(now_pair, lang)), true, lang),
                options: Vec::new(),
                selected: 0,
                recommended: None,
                values: value_lines(&group, &numbers, &now, lang),
                write_error: String::new(),
                baseline: text::restore_conflict_baseline(
                    &text::pair_name(pair(target, &group, &baseline), lang),
                    lang,
                ),
            }
        })
        .collect();
    number_same_names(&mut keyboards);
    let recommended = if put_back {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let selected = chosen.unwrap_or(recommended);
    ConflictPage {
        op_id: String::new(),
        operation: text::restore_operation(scope, name_of, lang),
        keyboards,
        error: String::new(),
        uac_line: crate::i18n::uac_line(lang),
        empty_note: String::new(),
        restore_note: text::restore_conflict_note(lang),
        restore_choices: RESTORE_POLICIES
            .iter()
            .map(|policy| {
                text::restore_conflict_choice(
                    *policy == ConflictPolicy::Overwrite,
                    *policy == recommended,
                    lang,
                )
            })
            .collect(),
        restore_selected: RESTORE_POLICIES
            .iter()
            .position(|policy| *policy == selected)
            .unwrap_or(0),
    }
}

/// [`ConflictKeyboard::target`] of `target`.
fn target_key(target: &WriteTarget) -> String {
    match target {
        WriteTarget::Device { instance_id } => instance_id.clone(),
        WriteTarget::Global => String::new(),
    }
}

/// Two collections of one device share a name: number them.
fn number_same_names(keyboards: &mut [ConflictKeyboard]) {
    for index in 0..keyboards.len() {
        let same: Vec<usize> = (0..keyboards.len())
            .filter(|other| keyboards[*other].name == keyboards[index].name)
            .collect();
        if same.len() > 1 && same[0] == index {
            for (number, other) in same.into_iter().enumerate() {
                keyboards[other].name = format!("{} ({})", keyboards[other].name, number + 1);
            }
        }
    }
}

/// Applies the user's choices so far: per keyboard the option index, per record an override.
pub fn apply_choices(
    keyboards: &mut [ConflictKeyboard],
    selections: &[(usize, usize)],
    overrides: &[(usize, Option<ResolutionChoice>)],
) {
    for (keyboard, option) in selections {
        if let Some(keyboard) = keyboards.get_mut(*keyboard)
            && *option < keyboard.options.len()
        {
            keyboard.selected = *option;
        }
    }
    for keyboard in keyboards.iter_mut() {
        for value in &mut keyboard.values {
            if let Some((_, choice)) = overrides.iter().find(|(record, _)| *record == value.record)
            {
                value.choice = *choice;
            }
        }
    }
}

/// The request's choices: every value of a keyboard gets the keyboard's option, unless the
/// details chose otherwise for that value.
pub fn choices(keyboards: &[ConflictKeyboard]) -> Vec<ValueChoice> {
    let mut choices: Vec<ValueChoice> = keyboards
        .iter()
        .flat_map(|keyboard| {
            let option = keyboard
                .options
                .get(keyboard.selected)
                .map_or(ResolutionChoice::KeepCurrent, |option| option.choice);
            keyboard.values.iter().map(move |value| ValueChoice {
                record: value.record,
                choice: value.choice.unwrap_or(option),
            })
        })
        .collect();
    choices.sort_by_key(|choice| choice.record);
    choices
}

/// The page for `entry` (the oldest entry in `Conflict`), with the keyboards already made by
/// [`conflict_keyboards`] and [`apply_choices`]; `None` when no entry is in conflict.
pub fn conflict_page(
    entry: Option<&JournalEntry>,
    keyboards: Vec<ConflictKeyboard>,
    error: Option<String>,
    name_of: NameOf<'_>,
    lang: Lang,
) -> ConflictPage {
    match entry {
        Some(entry) => ConflictPage {
            op_id: entry.op_id.to_string(),
            operation: kind_text(&entry.kind, name_of, lang),
            keyboards,
            error: error.unwrap_or_default(),
            uac_line: crate::i18n::uac_line(lang),
            ..ConflictPage::default()
        },
        None => ConflictPage {
            empty_note: text::conflict_nothing(lang),
            ..ConflictPage::default()
        },
    }
}

/// The helper refused the resolution for INV-PS2 (design m2 D.8): which PS/2 keyboards would be
/// left without a fixed layout, by name, and the ways out.
pub fn inv_ps2_error(keyboards: &[String], name_of: NameOf<'_>, lang: Lang) -> String {
    let names: Vec<String> = keyboards.iter().map(|id| name_of(id)).collect();
    let separator = match lang {
        Lang::Ja => "、",
        Lang::En => ", ",
    };
    text::conflict_inv_ps2(&names.join(separator), lang)
}

impl SnapshotText for ConflictPage {
    fn snapshot_text(&self) -> String {
        let mut out = String::new();
        if !self.empty_note.is_empty() {
            return format!("empty: {}\n", self.empty_note);
        }
        out.push_str(&format!("operation: {}\n", self.operation));
        if !self.restore_note.is_empty() {
            out.push_str(&format!("note: {}\n", self.restore_note));
        }
        if !self.error.is_empty() {
            out.push_str(&format!("error: {}\n", self.error));
        }
        for keyboard in &self.keyboards {
            out.push_str(&format!(
                "keyboard: {}\n  {}\n",
                keyboard.name, keyboard.now
            ));
            if !keyboard.baseline.is_empty() {
                out.push_str(&format!("  {}\n", keyboard.baseline));
            }
            for (index, option) in keyboard.options.iter().enumerate() {
                let mark = if index == keyboard.selected {
                    "◉"
                } else {
                    "○"
                };
                out.push_str(&format!("  {mark} {}\n", option.label));
            }
            if !keyboard.write_error.is_empty() {
                out.push_str(&format!("  {}\n", keyboard.write_error));
            }
            for value in &keyboard.values {
                out.push_str(&format!("  · {}\n", value.line));
            }
        }
        for (index, choice) in self.restore_choices.iter().enumerate() {
            let mark = if index == self.restore_selected {
                "◉"
            } else {
                "○"
            };
            out.push_str(&format!("{mark} {choice}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::OpState;

    use super::*;
    use crate::vm::test_journal::{KEYCHRON, entry};

    fn option(choice: ResolutionChoice) -> ConflictOption {
        ConflictOption {
            label: format!("{choice:?}"),
            choice,
        }
    }

    fn name_of(id: &str) -> String {
        if id.eq_ignore_ascii_case(KEYCHRON) {
            "Keychron Receiver".into()
        } else {
            id.into()
        }
    }

    /// The R6-like conflict: Keychron to JIS, then something else wrote 8/2.
    fn conflict() -> JournalEntry {
        let mut conflict = entry(
            "9a9a9a9a-0000-4000-8000-000000000010",
            16,
            OpState::Conflict,
        );
        conflict.records[0].conflict = Some(RegValue::Dword { value: 8 });
        conflict
    }

    fn now_8_2(record: &ValueRecord) -> Option<RegValue> {
        Some(RegValue::Dword {
            value: if record.name == value_names::HID_TYPE {
                8
            } else {
                2
            },
        })
    }

    fn page(
        entry: &JournalEntry,
        current: &dyn Fn(&ValueRecord) -> Option<RegValue>,
        lang: Lang,
    ) -> ConflictPage {
        let keyboards = conflict_keyboards(entry, current, &name_of, lang);
        conflict_page(Some(entry), keyboards, None, &name_of, lang)
    }

    #[test]
    fn recommendations_and_choices() {
        assert_eq!(
            recommend(Some(KeyboardType { ty: 8, subtype: 2 })),
            ResolutionChoice::UseBefore
        );
        assert_eq!(recommend(None), ResolutionChoice::UseBefore);
        assert_eq!(
            recommend(Some(KeyboardType::US)),
            ResolutionChoice::KeepCurrent
        );
        let keychron = ConflictKeyboard {
            target: String::new(),
            name: "Keychron Receiver".into(),
            now: String::new(),
            options: vec![
                option(ResolutionChoice::UseBefore),
                option(ResolutionChoice::UseIntended),
                option(ResolutionChoice::KeepCurrent),
            ],
            selected: 0,
            recommended: Some(0),
            values: vec![
                ConflictValue {
                    record: 1,
                    ..ConflictValue::default()
                },
                ConflictValue {
                    record: 0,
                    choice: Some(ResolutionChoice::UseBaseline),
                    ..ConflictValue::default()
                },
            ],
            write_error: String::new(),
            baseline: String::new(),
        };
        // Both values of the pair follow the keyboard's option, unless the details said
        // otherwise; a pair cannot be split by accident (review U6).
        assert_eq!(
            choices(&[keychron]),
            vec![
                ValueChoice {
                    record: 0,
                    choice: ResolutionChoice::UseBaseline
                },
                ValueChoice {
                    record: 1,
                    choice: ResolutionChoice::UseBefore
                },
            ]
        );
    }

    #[test]
    fn an_unknown_pair_recommends_the_value_before() {
        let entry = conflict();
        let ja = page(&entry, &now_8_2, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "operation: Keychron Receiver を JIS に\n\
             keyboard: Keychron Receiver\n  今の値: 不明な種類（8/2） — MKLM 以外が変更\n\
             \x20 ◉ 変更前の US に戻す（おすすめ）\n\
             \x20 ○ MKLM が設定した JIS にする\n\
             \x20 ○ 今の値のまま\n\
             \x20 · Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters\\KeyboardTypeOverride: \
             今の値 8 / MKLM が最後に書いた値 7 / 操作の前の値 4 / 操作で書こうとした値 7 / MKLM 導入前の値 4\n\
             \x20 · Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters\\KeyboardSubtypeOverride: \
             今の値 2 / MKLM が最後に書いた値 2 / 操作の前の値 0 / 操作で書こうとした値 2 / MKLM 導入前の値 0\n"
        );
        // The baseline equals the value before: no fourth option.
        assert_eq!(ja.keyboards[0].options.len(), 3);
        for text in [&ja.keyboards[0].now, &ja.keyboards[0].options[0].label] {
            assert!(
                crate::vm::unexpected_latin(text, &["Keychron Receiver"]).is_empty(),
                "{text}"
            );
        }
        assert_eq!(
            choices(&ja.keyboards),
            vec![
                ValueChoice {
                    record: 0,
                    choice: ResolutionChoice::UseBefore
                },
                ValueChoice {
                    record: 1,
                    choice: ResolutionChoice::UseBefore
                },
            ]
        );
        let en = page(&entry, &now_8_2, Lang::En);
        assert_eq!(
            en.keyboards[0].now,
            "Now: an unknown type (8/2) — changed outside MKLM"
        );
        assert_eq!(
            en.keyboards[0].options[0].label,
            "Back to US, as before the change (recommended)"
        );
    }

    #[test]
    fn a_valid_pair_set_elsewhere_is_kept_and_duplicates_go() {
        // The Settings app put US back (4/0 = the value before): "keep" and "back to US" write
        // the same, so only one of them is offered, and it is the recommended one.
        let entry = conflict();
        let us = |record: &ValueRecord| Some(record.before.clone());
        let mut keyboards = conflict_keyboards(&entry, &us, &name_of, Lang::Ja);
        let labels: Vec<&str> = keyboards[0]
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "変更前の US に戻す（おすすめ）",
                "MKLM が設定した JIS にする"
            ]
        );
        // A per-value override from the details, and the keyboard's second option.
        apply_choices(
            &mut keyboards,
            &[(0, 1)],
            &[(1, Some(ResolutionChoice::KeepCurrent))],
        );
        assert_eq!(
            choices(&keyboards),
            vec![
                ValueChoice {
                    record: 0,
                    choice: ResolutionChoice::UseIntended
                },
                ValueChoice {
                    record: 1,
                    choice: ResolutionChoice::KeepCurrent
                },
            ]
        );
        assert_eq!(value_choice_index(Some(ResolutionChoice::UseBaseline)), 4);
        assert_eq!(value_choice_index(None), 0);
    }

    #[test]
    fn unread_values_use_the_value_seen_at_the_conflict() {
        let mut entry = conflict();
        entry.records[1].conflict = Some(RegValue::Dword { value: 2 });
        entry.records[1].write_error = Some("Access is denied. (os error 5)".into());
        let unread = |_: &ValueRecord| None;
        let ja = page(&entry, &unread, Lang::Ja);
        let keychron = &ja.keyboards[0];
        assert_eq!(keychron.now, "今の値: 不明な種類（8/2） — MKLM 以外が変更");
        assert_eq!(
            keychron.write_error,
            "MKLM が書き込めなかった値があります。PC を再起動してからもう一度試してください。"
        );
        assert!(
            keychron.values[1]
                .line
                .ends_with("書き込みのエラー: Access is denied. (os error 5)")
        );
        // Nothing known at all: "読み取れません", and "keep" is never merged with another.
        let mut blind = conflict();
        blind.records[0].conflict = None;
        let ja = page(&blind, &unread, Lang::Ja);
        assert_eq!(ja.keyboards[0].now, "今の値: 読み取れません");
        assert_eq!(ja.keyboards[0].options.len(), 3);
    }

    #[test]
    fn the_pc_wide_values_and_inv_ps2() {
        let mut entry = conflict();
        for record in &mut entry.records {
            record.target = WriteTarget::Global;
            record.name = if record.name == value_names::HID_TYPE {
                value_names::PS2_TYPE.into()
            } else {
                value_names::PS2_SUBTYPE.into()
            };
            record.before = RegValue::Dword {
                value: if record.name == value_names::PS2_TYPE {
                    7
                } else {
                    2
                },
            };
            record.baseline = record.before.clone();
            record.intended = RegValue::Absent;
            record.last_written = Some(RegValue::Absent);
        }
        let now = |record: &ValueRecord| {
            Some(if record.name == value_names::PS2_TYPE {
                RegValue::Dword { value: 7 }
            } else {
                RegValue::Dword { value: 0 }
            })
        };
        let ja = page(&entry, &now, Lang::Ja);
        let global = &ja.keyboards[0];
        assert_eq!(global.name, "PC 全体の設定");
        assert_eq!(
            global.now,
            "今の値: 固定モード（不明な種類（7/0）） — MKLM 以外が変更"
        );
        assert_eq!(
            global.options[0].label,
            "変更前の固定モード（JIS）に戻す（おすすめ）"
        );
        assert_eq!(
            global.options[1].label,
            "MKLM が設定したキーボードごとモードにする"
        );
        assert_eq!(
            inv_ps2_error(
                &[crate::vm::test_journal::BUILT_IN.into()],
                &|_| "内蔵キーボード".into(),
                Lang::Ja
            ),
            "この選択では、配列が決まらない PS/2 キーボードが残ります（内蔵キーボード）。PC 全体の設定を「変更前」に戻す（固定モードに戻す）か、［確認待ちの変更をすべて元に戻す…］を選んでください。"
        );
        assert_eq!(
            conflict_page(None, Vec::new(), None, &name_of, Lang::Ja).snapshot_text(),
            "empty: MKLM 以外による変更は見つかっていません。\n"
        );
    }

    #[test]
    fn key_paths_name_their_targets() {
        let device = WriteTarget::Device {
            instance_id: KEYCHRON.into(),
        };
        for target in [device, WriteTarget::Global] {
            let path = ValueKey {
                target: target.clone(),
                name: value_names::HID_TYPE.into(),
            }
            .key_path();
            assert_eq!(target_of(&path), target);
            let lower = match &target {
                WriteTarget::Device { instance_id } => WriteTarget::Device {
                    instance_id: instance_id.to_ascii_lowercase(),
                },
                WriteTarget::Global => WriteTarget::Global,
            };
            assert_eq!(target_of(&path.to_ascii_lowercase()), lower);
        }
    }

    #[test]
    fn a_stopped_restore_of_the_pc_wide_values() {
        // "すべて導入前に戻す" found fixed JIS (7/2) set by something else where MKLM had removed
        // the values: a valid pair, so leaving it is recommended; before MKLM it was fixed US.
        let info = |name: &str, baseline: u32, current: u32| ConflictInfo {
            op_id: mklm_core::OpId::parse("9b9b9b9b-0000-4000-8000-000000000011").unwrap(),
            record: usize::from(name == value_names::PS2_SUBTYPE),
            key_path: ValueKey {
                target: WriteTarget::Global,
                name: name.into(),
            }
            .key_path(),
            name: name.into(),
            baseline: RegValue::Dword { value: baseline },
            before: RegValue::Dword { value: baseline },
            intended: RegValue::Absent,
            last_written: Some(RegValue::Absent),
            current: RegValue::Dword { value: current },
            write_error: None,
        };
        let conflicts = [
            info(value_names::PS2_TYPE, 4, 7),
            info(value_names::PS2_SUBTYPE, 0, 2),
        ];
        let unread = |_: &ValueRecord| None;
        let page = restore_conflict_page(
            &RestoreScope::All,
            &conflicts,
            None,
            &unread,
            &name_of,
            Lang::Ja,
        );
        assert_eq!(page.operation, "MKLM 導入前に戻す（すべて）");
        let global = &page.keyboards[0];
        assert_eq!(global.name, "PC 全体の設定");
        assert_eq!(global.now, "今の値: 固定モード（JIS） — MKLM 以外が変更");
        assert_eq!(global.baseline, "MKLM 導入前: 固定モード（US）");
        assert!(global.options.is_empty());
        assert_eq!(page.restore_selected, 0);
        assert_eq!(
            page.restore_choices[0],
            "MKLM 以外が変えた値はそのままにする（ほかの値は導入前に戻します）（おすすめ）"
        );
        assert!(page.can_resolve());
        for text in [&page.restore_note, &page.restore_choices[1], &global.now] {
            assert!(crate::vm::unexpected_latin(text, &[]).is_empty(), "{text}");
        }
        // The user's pick wins over the recommendation.
        let chosen = restore_conflict_page(
            &RestoreScope::All,
            &conflicts,
            Some(ConflictPolicy::Overwrite),
            &unread,
            &name_of,
            Lang::En,
        );
        assert_eq!(chosen.restore_selected, 1);
        assert_eq!(
            chosen.restore_choices,
            [
                "Leave those values as they are (the others go back to before MKLM) (recommended)",
                "Put those values back to before MKLM too",
            ]
        );
    }
}
