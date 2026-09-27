//! Offline recovery files (plan 2.3 "helper がないときの回復"): a `.reg` file and
//! `restore-offline.cmd` that put every value MKLM changed back to its baseline, using this PC's
//! real instance paths. Rendering is pure; `mklm-engine` writes the files into
//! `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` through `mklm-win`.
//!
//! Formats (section G of docs/design/m2-engine.md):
//! - `.reg`: UTF-16LE with BOM, `Windows Registry Editor Version 5.00`, keys under
//!   `HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\…`, `"name"=dword:…`, `"name"="…"`, `"name"=-`.
//! - `.cmd`: ASCII, CRLF, `restore-offline.cmd online` or `restore-offline.cmd offline D:` (WinRE:
//!   loads `D:\Windows\System32\config\SYSTEM`, resolves `Select\Default`, the control set the
//!   next boot uses, to `ControlSet00N`, and stops with a message when `Select\Current` differs;
//!   design review C11).
//!
//! Order (both files). The files do not know the current values, so they use one static order
//! that is safe from any starting point (design review C4): i8042prt values that the baseline sets,
//! then global values it sets, then HID values, then global values it deletes, then i8042prt values
//! it deletes. Pins and the fixed pair are therefore present before anything that relies on them
//! is removed; a power loss in the middle of the script cannot break INV-PS2 unless the baseline
//! itself does.
//!
//! Every string that reaches the `.cmd` must pass [`is_cmd_safe`]; a record that does not is left
//! out with a `rem` line that names only its position, never its text (a USB serial number ends up
//! in instance IDs and must not be able to inject commands).
//!
//! Records are numbered `#1`, `#2`, … in input order in all three files. Inside each group of the
//! order above they are sorted by key path and value name, so the output does not depend on the
//! order the store listed them in.

use serde::{Deserialize, Serialize};

use crate::allowlist::{DEVICE_VALUE_NAMES, GLOBAL_VALUE_NAMES, WriteTarget};
use crate::journal::{BaselineRecord, RegValue, Timestamp, is_ps2_value_name};

/// File name of the batch file.
pub const RESTORE_CMD_FILE: &str = "restore-offline.cmd";
/// File name of the `.reg` file.
pub const BASELINE_REG_FILE: &str = "mklm-baseline.reg";
/// File name of the explanation next to them (UTF-8 with BOM, Japanese and English).
pub const README_FILE: &str = "README.txt";
/// Hive mount point `restore-offline.cmd offline` uses in WinRE.
pub const OFFLINE_HIVE_MOUNT: &str = r"HKLM\MKLM_OFFLINE";

/// Rendered recovery files, ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAssets {
    /// [`BASELINE_REG_FILE`] content (UTF-16LE with BOM).
    pub reg: Vec<u8>,
    /// [`RESTORE_CMD_FILE`] content (ASCII, CRLF).
    pub cmd: String,
    /// [`README_FILE`] content.
    pub readme: String,
    /// Records left out because they failed [`is_cmd_safe`] (indices into the input).
    ///
    /// More precisely: every record `restore-offline.cmd` does not restore, which also covers a
    /// name MKLM never writes, malformed raw data, and a [`RegValue::Other`] type other than
    /// `REG_BINARY` (the `.reg` still restores those it can express).
    pub skipped: Vec<usize>,
}

/// Rendering failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AssetError {
    #[error("no baseline records to render")]
    Empty,
}

/// True when `text` contains only characters that are inert inside a double-quoted `cmd.exe`
/// argument with delayed expansion off: ASCII letters, digits, space and `_ - . \ & # { } ( ) , ; =`.
/// Rejects `"`, `%`, `!`, `^`, `<`, `>`, `|`, control characters and anything non-ASCII.
pub fn is_cmd_safe(text: &str) -> bool {
    text.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                ' ' | '_' | '-' | '.' | '\\' | '&' | '#' | '{' | '}' | '(' | ')' | ',' | ';' | '='
            )
    })
}

/// True when `text` can stand in a `.reg` file: no control characters (a line break would end the
/// line early).
fn is_reg_text_safe(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

/// Registry path of the control set the running Windows uses, as the `.reg` file names it.
const REG_ROOT: &str = r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet";

/// The fixed order of both files (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Ps2Set,
    GlobalSet,
    Hid,
    GlobalDeleted,
    Ps2Deleted,
}

impl Group {
    const ALL: [Group; 5] = [
        Group::Ps2Set,
        Group::GlobalSet,
        Group::Hid,
        Group::GlobalDeleted,
        Group::Ps2Deleted,
    ];

    fn label(self) -> &'static str {
        match self {
            Group::Ps2Set => "PS/2 values set",
            Group::GlobalSet => "global values set",
            Group::Hid => "HID keyboards",
            Group::GlobalDeleted => "global values deleted",
            Group::Ps2Deleted => "PS/2 values deleted",
        }
    }
}

/// Where a record goes, or `None` for a name MKLM never writes on that key.
fn group_of(record: &BaselineRecord) -> Option<Group> {
    let name = record.key.name.as_str();
    let deletes = record.value == RegValue::Absent;
    match &record.key.target {
        WriteTarget::Global if GLOBAL_VALUE_NAMES.contains(&name) => Some(if deletes {
            Group::GlobalDeleted
        } else {
            Group::GlobalSet
        }),
        WriteTarget::Device { .. } if DEVICE_VALUE_NAMES.contains(&name) => {
            Some(match (is_ps2_value_name(name), deletes) {
                (true, false) => Group::Ps2Set,
                (true, true) => Group::Ps2Deleted,
                (false, _) => Group::Hid,
            })
        }
        _ => None,
    }
}

/// Position of a value name in the allowlists, so that a type comes before its subtype.
fn name_rank(name: &str) -> usize {
    GLOBAL_VALUE_NAMES
        .iter()
        .chain(DEVICE_VALUE_NAMES.iter())
        .position(|n| *n == name)
        .unwrap_or(usize::MAX)
}

/// Raw data of a [`RegValue::Other`], when its hex text is well formed.
fn other_bytes(data_hex: &str) -> Option<Vec<u8>> {
    if !data_hex.len().is_multiple_of(2) {
        return None;
    }
    (0..data_hex.len())
        .step_by(2)
        .map(|i| {
            data_hex
                .get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect()
}

/// One record in output order.
struct Item<'a> {
    /// 1-based position in the input.
    number: usize,
    index: usize,
    group: Group,
    key_path: String,
    record: &'a BaselineRecord,
}

/// `"text"` for a `.reg` file.
fn reg_quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', r"\\").replace('"', "\\\""))
}

/// The `.reg` line for one value, or `None` when the value cannot be expressed safely.
fn reg_value_line(name: &str, value: &RegValue) -> Option<String> {
    let name = reg_quote(name);
    Some(match value {
        RegValue::Absent => format!("{name}=-"),
        RegValue::Dword { value } => format!("{name}=dword:{value:08x}"),
        RegValue::Sz { value } if is_reg_text_safe(value) => format!("{name}={}", reg_quote(value)),
        RegValue::Sz { .. } => return None,
        RegValue::Other { reg_type, data_hex } => {
            let bytes = other_bytes(data_hex)?;
            let data: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
            // `hex:` is how regedit writes REG_BINARY; every other type is `hex(<type>):`.
            let kind = if *reg_type == 3 {
                "hex".to_string()
            } else {
                format!("hex({reg_type:x})")
            };
            format!("{name}={kind}:{}", data.join(","))
        }
    })
}

/// The `.cmd` line that restores one value, or the reason the script leaves it out.
fn cmd_line(item: &Item<'_>) -> Result<String, String> {
    let number = item.number;
    let unsafe_text = || {
        format!(
            "Record #{number} was left out: it contains characters that are not safe in cmd.exe. \
             See {BASELINE_REG_FILE}."
        )
    };
    let name = &item.record.key.name;
    if !is_cmd_safe(&item.key_path) || !is_cmd_safe(name) {
        return Err(unsafe_text());
    }
    let key = format!("\"%ROOT%\\{}\"", item.key_path);
    let add = |kind: &str, data: Option<String>| {
        let data = data.map(|d| format!(" /d {d}")).unwrap_or_default();
        Ok(format!(
            "reg add {key} /v \"{name}\" /t {kind}{data} /f >nul || set \"FAILED=1\""
        ))
    };
    match &item.record.value {
        RegValue::Absent => Ok(format!("call :del {key} \"{name}\"")),
        RegValue::Dword { value } => add("REG_DWORD", Some(value.to_string())),
        // A trailing backslash would escape the closing quote in reg.exe's argument parsing.
        RegValue::Sz { value } if is_cmd_safe(value) && !value.ends_with('\\') => {
            add("REG_SZ", Some(format!("\"{value}\"")))
        }
        RegValue::Sz { .. } => Err(unsafe_text()),
        RegValue::Other {
            reg_type: 3,
            data_hex,
        } => match other_bytes(data_hex) {
            Some(bytes) => {
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                add("REG_BINARY", (!hex.is_empty()).then_some(hex))
            }
            None => Err(format!(
                "Record #{number} was left out: its recorded data is malformed."
            )),
        },
        RegValue::Other { reg_type, .. } => Err(format!(
            "Record #{number} has registry type {reg_type}, which this script cannot write. \
             Use {BASELINE_REG_FILE}."
        )),
    }
}

/// `YYYY-MM-DD HH:MM UTC`.
fn format_utc(at: Timestamp) -> String {
    let seconds = at.0 / 1000;
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let minutes = seconds % 86_400 / 60;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        minutes / 60,
        minutes % 60
    )
}

/// Proleptic Gregorian date of a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Text of one value for the README.
fn describe(value: &RegValue) -> String {
    match value {
        RegValue::Absent => "(値なし: 削除 / absent: delete)".to_string(),
        RegValue::Dword { value } => format!("REG_DWORD {value} (0x{value:x})"),
        RegValue::Sz { value } => format!("REG_SZ \"{}\"", printable(value)),
        RegValue::Other { reg_type, data_hex } => {
            format!("registry type {reg_type}, hex {}", printable(data_hex))
        }
    }
}

/// `text` with control characters replaced, for the README (it is only read, never run).
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn crlf(lines: &[String]) -> String {
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    text
}

/// Renders the three files from the baseline store.
pub fn render_recovery_assets(
    baselines: &[BaselineRecord],
    generator_version: &str,
    generated_at: Timestamp,
) -> Result<RecoveryAssets, AssetError> {
    if baselines.is_empty() {
        return Err(AssetError::Empty);
    }
    // The version lands on `rem` lines, outside quotes: letters, digits, `.`, `-` and `_` only.
    let plain_version = !generator_version.is_empty()
        && generator_version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    let version = if plain_version {
        generator_version
    } else {
        "(unknown version)"
    };
    let when = format_utc(generated_at);

    let mut unknown = Vec::new();
    let mut items: Vec<Item<'_>> = Vec::new();
    for (index, record) in baselines.iter().enumerate() {
        match group_of(record) {
            Some(group) => items.push(Item {
                number: index + 1,
                index,
                group,
                key_path: record.key.key_path(),
                record,
            }),
            None => unknown.push(index),
        }
    }
    items.sort_by(|a, b| {
        (
            a.group,
            a.key_path.to_ascii_uppercase(),
            name_rank(&a.record.key.name),
        )
            .cmp(&(
                b.group,
                b.key_path.to_ascii_uppercase(),
                name_rank(&b.record.key.name),
            ))
    });

    // .reg
    let mut reg = vec![
        "Windows Registry Editor Version 5.00".to_string(),
        String::new(),
        format!("; MKLM baseline (values before MKLM changed them), generated {when} by MKLM {version}"),
        "; Order: PS/2 values set, global values set, HID keyboards, global values deleted, PS/2 values deleted.".to_string(),
    ];
    let mut first_group = true;
    for group in Group::ALL {
        let members: Vec<&Item<'_>> = items.iter().filter(|i| i.group == group).collect();
        if members.is_empty() {
            continue;
        }
        if !std::mem::take(&mut first_group) {
            reg.push(String::new());
        }
        reg.push(format!("; {}", group.label()));
        let mut open_key: Option<&str> = None;
        for item in members {
            let line = reg_value_line(&item.record.key.name, &item.record.value);
            let key_ok = is_reg_text_safe(&item.key_path)
                && !item.key_path.contains(['[', ']'])
                && is_reg_text_safe(&item.record.key.name);
            match line {
                Some(line) if key_ok => {
                    if open_key != Some(item.key_path.as_str()) {
                        if open_key.is_some() {
                            reg.push(String::new());
                        }
                        reg.push(format!("[{REG_ROOT}\\{}]", item.key_path));
                        open_key = Some(&item.key_path);
                    }
                    reg.push(line);
                }
                _ => reg.push(format!(
                    "; Record #{} was left out: it cannot be written safely in this file.",
                    item.number
                )),
            }
        }
    }
    for index in &unknown {
        reg.push(format!(
            "; Record #{} was left out: MKLM does not write that value.",
            index + 1
        ));
    }
    reg.push(String::new());
    let reg_text = crlf(&reg);
    let mut reg_bytes = vec![0xFF, 0xFE];
    for unit in reg_text.encode_utf16() {
        reg_bytes.extend_from_slice(&unit.to_le_bytes());
    }

    // .cmd
    let mut skipped: Vec<usize> = unknown.clone();
    let mut apply = vec!["set \"FAILED=\"".to_string()];
    for group in Group::ALL {
        let members: Vec<&Item<'_>> = items.iter().filter(|i| i.group == group).collect();
        if members.is_empty() {
            continue;
        }
        apply.push(format!("rem {}", group.label()));
        for item in members {
            match cmd_line(item) {
                Ok(line) => apply.push(line),
                Err(reason) => {
                    // Only the position is named, never the text (it may be crafted).
                    apply.push(format!("rem {reason}"));
                    apply.push("set \"FAILED=1\"".to_string());
                    skipped.push(item.index);
                }
            }
        }
    }
    for index in &unknown {
        apply.push(format!(
            "rem Record #{} was left out: MKLM does not write that value.",
            index + 1
        ));
        apply.push("set \"FAILED=1\"".to_string());
    }
    skipped.sort_unstable();

    let mut cmd: Vec<String> = vec![
        "@echo off".to_string(),
        format!("rem MKLM recovery script, generated {when} by MKLM {version}."),
        "rem Puts every keyboard value MKLM changed back to its value before MKLM (baseline).".to_string(),
        "rem   restore-offline.cmd online        Windows or Safe Mode, from an administrator prompt".to_string(),
        "rem   restore-offline.cmd offline D:    WinRE command prompt; D: is the Windows drive".to_string(),
        "rem Run this copy (on the Windows drive). A copy on a USB stick is for reading only.".to_string(),
    ];
    cmd.extend(CMD_DISPATCH.iter().map(|l| l.to_string()));
    cmd.push(String::new());
    cmd.push(":apply".to_string());
    cmd.extend(apply);
    cmd.extend(CMD_TAIL.iter().map(|l| l.to_string()));

    Ok(RecoveryAssets {
        reg: reg_bytes,
        cmd: crlf(&cmd),
        readme: render_readme(&items, &unknown, version, &when),
        skipped,
    })
}

/// The fixed part of `restore-offline.cmd` between the header and `:apply` (design G.3).
const CMD_DISPATCH: &[&str] = &[
    "setlocal EnableExtensions DisableDelayedExpansion",
    r#"if /i "%~1"=="online" goto :online"#,
    r#"if /i "%~1"=="offline" goto :offline"#,
    "goto :usage",
    "",
    ":online",
    r#"set "ROOT=HKLM\SYSTEM\CurrentControlSet""#,
    "call :apply",
    "exit /b %ERRORLEVEL%",
    "",
    ":offline",
    r#"if "%~2"=="" goto :usage"#,
    r#"if not exist "%~2\Windows\System32\config\SYSTEM" (echo SYSTEM hive not found under %~2\Windows& exit /b 2)"#,
    r#"reg load HKLM\MKLM_OFFLINE "%~2\Windows\System32\config\SYSTEM" >nul || (echo reg load failed& exit /b 3)"#,
    r#"set "CS=""#,
    r#"set "CUR=""#,
    r#"for /f "tokens=3" %%A in ('reg query HKLM\MKLM_OFFLINE\Select /v Default ^| findstr /c:"Default"') do set /a CS=%%A"#,
    r#"for /f "tokens=3" %%A in ('reg query HKLM\MKLM_OFFLINE\Select /v Current ^| findstr /c:"Current"') do set /a CUR=%%A"#,
    r"if not defined CS (reg unload HKLM\MKLM_OFFLINE >nul & echo Select\Default not found& exit /b 4)",
    r#"if not "%CS%"=="%CUR%" goto :cs_mismatch"#,
    r#"set "CSN=00%CS%""#,
    r#"set "ROOT=HKLM\MKLM_OFFLINE\ControlSet%CSN:~-3%""#,
    "call :apply",
    r#"set "RC=%ERRORLEVEL%""#,
    r"reg unload HKLM\MKLM_OFFLINE >nul",
    "exit /b %RC%",
    "",
    ":cs_mismatch",
    r"reg unload HKLM\MKLM_OFFLINE >nul",
    r"echo Select\Default is %CS% but Select\Current is %CUR%. Nothing was changed; see docs\recovery.md.",
    "exit /b 5",
];

/// The fixed part of `restore-offline.cmd` after the generated `:apply` lines (design G.3).
const CMD_TAIL: &[&str] = &[
    "if defined FAILED (echo Some values could not be restored.& exit /b 1)",
    "echo Done. Restart Windows (a restart, not a shutdown).",
    "exit /b 0",
    "",
    ":del",
    r#"reg delete "%~1" /v "%~2" /f >nul 2>&1"#,
    r#"reg query "%~1" /v "%~2" >nul 2>&1 && set "FAILED=1""#,
    "exit /b 0",
    "",
    ":usage",
    "echo usage: restore-offline.cmd online ^| offline D:",
    "exit /b 2",
];

/// `README.txt` (design G.4): UTF-8 with BOM, CRLF, Japanese then English.
fn render_readme(items: &[Item<'_>], unknown: &[usize], version: &str, when: &str) -> String {
    let mut lines: Vec<String> = vec![
        "\u{feff}MKLM 復旧用ファイル / MKLM recovery files".to_string(),
        format!("生成: {when}、MKLM {version} / Generated {when} by MKLM {version}"),
        String::new(),
        "■ 最初に試すこと".to_string(),
        "1. コマンドプロンプト（管理者でなくてよい）で mklm-cli undo を実行する。確認待ち、再起動待ち、衝突の変更をまとめて元に戻します（UAC の確認が出ます）。".to_string(),
        "2. それで直らなければ mklm-cli recover、次に mklm-cli restore --baseline --all を実行する。".to_string(),
        "3. 最後に PC を「再起動」する（「シャットダウン」ではなく「再起動」）。".to_string(),
        String::new(),
        "■ このフォルダーのファイル".to_string(),
        format!("- {RESTORE_CMD_FILE}: MKLM が変えたキーボードの値を、MKLM 導入前の値に戻すスクリプト。"),
        format!("    {RESTORE_CMD_FILE} online       Windows かセーフモードの、管理者のコマンドプロンプトで実行"),
        format!("    {RESTORE_CMD_FILE} offline D:   回復環境（WinRE）のコマンドプロンプトで実行。D: は Windows のドライブ"),
        format!("- {BASELINE_REG_FILE}: 同じ内容のレジストリファイル。Windows 上でダブルクリックして取り込む（UAC の確認が出ます）。"),
        format!("- {README_FILE}: このファイル。"),
        String::new(),
        "■ 回復環境（WinRE）での手順（詳しくは docs/recovery.md）".to_string(),
        "1. サインイン画面の電源ボタン → Shift を押しながら「再起動」→ トラブルシューティング → 詳細オプション → コマンド プロンプト。".to_string(),
        "2. BitLocker の回復キーを求められたら入力する（https://aka.ms/myrecoverykey で確認できます）。".to_string(),
        "3. Windows のドライブを確かめる（WinRE では D: のことが多い。dir D:\\Windows）。".to_string(),
        "4. cd /d \"D:\\ProgramData\\SHIN DATA CENTER\\MKLM\\Recovery\"".to_string(),
        format!("5. {RESTORE_CMD_FILE} offline D:"),
        "   \" のキーは、JIS 配列では Shift+2、US 配列では Shift+'（Enter の左）。".to_string(),
        "6. exit と入力し、「続行」で Windows を起動する。".to_string(),
        String::new(),
        "■ USB メモリへのコピー".to_string(),
        "このフォルダーを USB メモリなどにコピーしておくと、インスタンス ID と手順を読むための控えになります。".to_string(),
        format!("ただし、実行するのはディスク上の ...\\ProgramData\\SHIN DATA CENTER\\MKLM\\Recovery\\{RESTORE_CMD_FILE} だけにしてください。USB メモリ上のファイルは書き換えられるおそれがあり、WinRE では SYSTEM 権限で実行されるためです。"),
        String::new(),
        "■ 記録した値（MKLM 導入前の値）".to_string(),
    ];
    let values = value_list(items, unknown);
    lines.extend(values.iter().cloned());
    lines.extend([
        String::new(),
        "---- English ----".to_string(),
        String::new(),
        "First try:".to_string(),
        "1. In a command prompt (no administrator needed), run mklm-cli undo. It reverts every change that waits for a confirmation or a restart, or is in conflict (a UAC prompt appears).".to_string(),
        "2. If that does not help, run mklm-cli recover, then mklm-cli restore --baseline --all.".to_string(),
        "3. Then restart the PC (a restart, not a shutdown).".to_string(),
        String::new(),
        "Files in this folder:".to_string(),
        format!("- {RESTORE_CMD_FILE}: puts every keyboard value MKLM changed back to its value before MKLM."),
        format!("    {RESTORE_CMD_FILE} online       from an administrator prompt in Windows or Safe Mode"),
        format!("    {RESTORE_CMD_FILE} offline D:   from the WinRE command prompt; D: is the Windows drive"),
        format!("- {BASELINE_REG_FILE}: the same values as a registry file; double-click it in Windows (a UAC prompt appears)."),
        format!("- {README_FILE}: this file."),
        String::new(),
        "In the recovery environment (WinRE; details in docs/recovery.md):".to_string(),
        "1. On the sign-in screen, hold Shift and choose Power > Restart, then Troubleshoot > Advanced options > Command Prompt.".to_string(),
        "2. Enter the BitLocker recovery key if asked (see https://aka.ms/myrecoverykey).".to_string(),
        "3. Find the Windows drive (often D: in WinRE: dir D:\\Windows).".to_string(),
        "4. cd /d \"D:\\ProgramData\\SHIN DATA CENTER\\MKLM\\Recovery\"".to_string(),
        format!("5. {RESTORE_CMD_FILE} offline D:"),
        "   The \" key is Shift+2 on a JIS keyboard and Shift+' (left of Enter) on a US keyboard.".to_string(),
        "6. Type exit and choose Continue to start Windows.".to_string(),
        String::new(),
        "Copying to a USB stick:".to_string(),
        "A copy of this folder is a reference for the instance IDs and the steps. Only ever run".to_string(),
        format!("...\\ProgramData\\SHIN DATA CENTER\\MKLM\\Recovery\\{RESTORE_CMD_FILE} on the Windows drive: a file on a USB stick can be altered, and WinRE runs it with SYSTEM rights."),
        String::new(),
        "Recorded values (before MKLM): see the list above.".to_string(),
    ]);
    crlf(&lines)
}

/// The README's list of every recorded value, in output order.
fn value_list(items: &[Item<'_>], unknown: &[usize]) -> Vec<String> {
    let mut lines = Vec::new();
    for item in items {
        lines.push(format!(
            "#{} HKLM\\SYSTEM\\CurrentControlSet\\{}",
            item.number,
            printable(&item.key_path)
        ));
        lines.push(format!(
            "   {} = {}",
            printable(&item.record.key.name),
            describe(&item.record.value)
        ));
    }
    for index in unknown {
        lines.push(format!(
            "#{} (MKLM が書かない値のため省略 / left out: not a value MKLM writes)",
            index + 1
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::journal::{BASELINE_SCHEMA_VERSION, OpId, ValueKey};
    use crate::model::value_names::*;

    /// 2026-09-27 04:00 UTC, as in the design's samples.
    const GENERATED_AT: Timestamp = Timestamp(1_790_481_600_000);

    fn baseline(target: WriteTarget, name: &str, value: RegValue) -> BaselineRecord {
        let key = ValueKey {
            target,
            name: name.into(),
        };
        BaselineRecord {
            schema_version: BASELINE_SCHEMA_VERSION,
            key_path: key.key_path(),
            key,
            value,
            captured_at: GENERATED_AT,
            captured_by: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
        }
    }

    fn device(instance_id: &str) -> WriteTarget {
        WriteTarget::Device {
            instance_id: instance_id.into(),
        }
    }

    fn dword(value: u32) -> RegValue {
        RegValue::Dword { value }
    }

    /// Before M0: fixed JIS, nothing stored per device. What MKLM records when it migrates such a
    /// PC (the sample of design G.2 and G.3). Listed in store order, not output order.
    fn before_m0() -> Vec<BaselineRecord> {
        let keychron = fixtures::keychron().instance_id;
        let ps2 = fixtures::internal_ps2().instance_id;
        vec![
            baseline(device(&keychron), HID_TYPE, RegValue::Absent),
            baseline(device(&ps2), PS2_SUBTYPE, RegValue::Absent),
            baseline(WriteTarget::Global, PS2_SUBTYPE, dword(2)),
            baseline(device(&keychron), HID_SUBTYPE, RegValue::Absent),
            baseline(device(&ps2), PS2_TYPE, RegValue::Absent),
            baseline(WriteTarget::Global, PS2_TYPE, dword(7)),
        ]
    }

    /// The development machine after M0 (design C.6): Keychron 4/0, the internal keyboard pinned
    /// 7/2, no global pair.
    fn dev_machine() -> Vec<BaselineRecord> {
        let keychron = fixtures::keychron().instance_id;
        let ps2 = fixtures::internal_ps2().instance_id;
        vec![
            baseline(WriteTarget::Global, PS2_TYPE, RegValue::Absent),
            baseline(WriteTarget::Global, PS2_SUBTYPE, RegValue::Absent),
            baseline(device(&keychron), HID_TYPE, dword(4)),
            baseline(device(&keychron), HID_SUBTYPE, dword(0)),
            baseline(device(&ps2), PS2_TYPE, dword(7)),
            baseline(device(&ps2), PS2_SUBTYPE, dword(2)),
        ]
    }

    fn reg_text(assets: &RecoveryAssets) -> String {
        assert_eq!(&assets.reg[..2], &[0xFF, 0xFE], "UTF-16LE BOM");
        let units: Vec<u16> = assets.reg[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        assert!(assets.reg.len().is_multiple_of(2));
        String::from_utf16(&units).unwrap()
    }

    /// Every line ends with CRLF, and there is no lone LF or CR.
    fn assert_crlf(text: &str) {
        assert!(text.ends_with("\r\n"));
        let bytes = text.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                assert!(i > 0 && bytes[i - 1] == b'\r', "lone LF at {i}");
            }
            if *b == b'\r' {
                assert_eq!(bytes.get(i + 1), Some(&b'\n'), "lone CR at {i}");
            }
        }
    }

    /// Compares with `testdata/recovery/<name>` (line endings normalized, so that a checkout that
    /// converts them does not matter). `MKLM_UPDATE_GOLDEN=1` rewrites the file instead.
    fn golden(name: &str, actual: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("recovery")
            .join(name);
        let normalized = actual.replace("\r\n", "\n");
        if std::env::var_os("MKLM_UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &normalized).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; run with MKLM_UPDATE_GOLDEN=1", path.display()))
            .replace("\r\n", "\n");
        assert_eq!(normalized, expected, "{name} differs from the golden file");
    }

    #[test]
    fn golden_files() {
        for (name, baselines) in [("before-m0", before_m0()), ("dev-machine", dev_machine())] {
            let assets = render_recovery_assets(&baselines, "0.1.0", GENERATED_AT).unwrap();
            assert!(assets.skipped.is_empty(), "{name}");
            let reg = reg_text(&assets);
            assert_crlf(&reg);
            assert_crlf(&assets.cmd);
            assert_crlf(&assets.readme);
            assert!(assets.cmd.is_ascii());
            golden(&format!("{name}.reg.txt"), &reg);
            golden(&format!("{name}.cmd"), &assets.cmd);
            golden(&format!("{name}.README.txt"), &assets.readme);
        }
    }

    /// Group comments in the order they appear.
    fn groups_in(text: &str, prefix: &str) -> Vec<String> {
        let labels = Group::ALL.map(Group::label);
        text.lines()
            .filter_map(|l| l.strip_prefix(prefix))
            .filter(|l| labels.contains(l))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn fixed_order_whatever_the_store_order() {
        let keychron = fixtures::keychron().instance_id;
        let ps2 = fixtures::internal_ps2().instance_id;
        let phantom = r"ACPI\PNP0303\4&1&0";
        // One record per group, listed backwards.
        let baselines = vec![
            baseline(device(phantom), PS2_TYPE, RegValue::Absent),
            baseline(WriteTarget::Global, PS2_TYPE, RegValue::Absent),
            baseline(device(&keychron), HID_TYPE, dword(4)),
            baseline(
                WriteTarget::Global,
                LAYER_DRIVER_JPN,
                RegValue::Sz {
                    value: "kbd106.dll".into(),
                },
            ),
            baseline(device(&ps2), PS2_TYPE, dword(7)),
        ];
        let assets = render_recovery_assets(&baselines, "0.1.0", GENERATED_AT).unwrap();
        let order = [
            "PS/2 values set",
            "global values set",
            "HID keyboards",
            "global values deleted",
            "PS/2 values deleted",
        ];
        assert_eq!(groups_in(&reg_text(&assets), "; "), order);
        assert_eq!(groups_in(&assets.cmd, "rem "), order);
        let reg = reg_text(&assets);
        let position = |needle: &str| reg.find(needle).unwrap();
        assert!(
            position("\"LayerDriver JPN\"=\"kbd106.dll\"")
                < position("\"KeyboardTypeOverride\"=dword:00000004")
        );
        assert!(assets.cmd.contains(
            r#"reg add "%ROOT%\Services\i8042prt\Parameters" /v "LayerDriver JPN" /t REG_SZ /d "kbd106.dll" /f >nul || set "FAILED=1""#
        ));
        // README lists the records with their input numbers.
        assert!(
            assets
                .readme
                .contains("#5 HKLM\\SYSTEM\\CurrentControlSet\\Enum\\ACPI\\FUJ0309")
        );
    }

    #[test]
    fn offline_mode_uses_select_default_and_stops_when_current_differs() {
        let assets = render_recovery_assets(&before_m0(), "0.1.0", GENERATED_AT).unwrap();
        let cmd = &assets.cmd;
        assert!(cmd.contains(r"reg query HKLM\MKLM_OFFLINE\Select /v Default"));
        assert!(cmd.contains(r#"if not "%CS%"=="%CUR%" goto :cs_mismatch"#));
        assert!(cmd.contains(r#"set "ROOT=HKLM\MKLM_OFFLINE\ControlSet%CSN:~-3%""#));
        let mismatch = cmd.find("\r\n:cs_mismatch\r\n").unwrap() + 2;
        assert!(cmd[mismatch..].starts_with(
            ":cs_mismatch\r\nreg unload HKLM\\MKLM_OFFLINE >nul\r\necho Select\\Default is %CS% but Select\\Current is %CUR%. Nothing was changed"
        ));
        assert!(cmd[mismatch..].contains("exit /b 5"));
        assert!(cmd.contains(OFFLINE_HIVE_MOUNT));
        // The mismatch branch is reached before any value is written.
        assert!(
            cmd.find("goto :cs_mismatch").unwrap() < cmd.find("call :apply\r\nset \"RC").unwrap()
        );
    }

    #[test]
    fn deletion_syntax() {
        let assets = render_recovery_assets(&before_m0(), "0.1.0", GENERATED_AT).unwrap();
        let reg = reg_text(&assets);
        assert!(reg.contains("\r\n\"KeyboardTypeOverride\"=-\r\n"));
        assert!(reg.contains("\r\n\"OverrideKeyboardSubtype\"=-\r\n"));
        assert!(assets.cmd.contains(
            r#"call :del "%ROOT%\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters" "OverrideKeyboardType""#
        ));
        assert!(
            assets
                .cmd
                .contains(r#"reg query "%~1" /v "%~2" >nul 2>&1 && set "FAILED=1""#)
        );
        assert!(reg.starts_with("Windows Registry Editor Version 5.00\r\n\r\n"));
        assert!(reg.contains(
            "[HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Services\\i8042prt\\Parameters]\r\n\"OverrideKeyboardType\"=dword:00000007\r\n"
        ));
    }

    #[test]
    fn is_cmd_safe_table() {
        for safe in [
            r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000",
            r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001",
            r"ACPI\FUJ0309\4&320DB4C2&0",
            "Device Parameters",
            "LayerDriver JPN",
            "kbd106.dll",
            "a,b;c=d#e(f)",
            "",
        ] {
            assert!(is_cmd_safe(safe), "{safe}");
        }
        for injected in [
            r#""&calc&""#,
            "%PATH%",
            "a^b",
            "!x!",
            "a<b",
            "a>b",
            "a|b",
            "a\"b",
            "line\r\nbreak",
            "tab\there",
            "nul\0",
            "キーボード",
            "caf\u{e9}",
            "a[b]",
            "a'b",
            "a`b",
            "a/b",
            "a*b",
            "a?b",
            "a:b",
            "a+b",
            "a@b",
            "a$b",
            "a~b",
            "\u{ff05}PATH\u{ff05}",
        ] {
            assert!(!is_cmd_safe(injected), "{injected:?}");
        }
    }

    #[test]
    fn unsafe_records_are_left_out_by_number_only() {
        let crafted = r"USB\VID_1234&PID_5678\SERIAL%PATH%&calc";
        let baselines = vec![
            baseline(
                device(&fixtures::keychron().instance_id),
                HID_TYPE,
                dword(4),
            ),
            baseline(device(crafted), HID_TYPE, dword(4)),
            baseline(device("HID\\LINE\r\nBREAK\\1"), HID_SUBTYPE, dword(0)),
        ];
        let assets = render_recovery_assets(&baselines, "0.1.0", GENERATED_AT).unwrap();
        assert_eq!(assets.skipped, vec![1, 2]);
        assert!(!assets.cmd.contains("SERIAL"));
        assert!(!assets.cmd.contains("BREAK"));
        assert!(assets.cmd.contains("rem Record #2 was left out"));
        assert!(assets.cmd.contains("rem Record #3 was left out"));
        assert!(assets.cmd.contains("Some values could not be restored."));
        let reg = reg_text(&assets);
        // The .reg can still carry the crafted serial (nothing expands there) ...
        assert!(reg.contains("SERIAL%PATH%&calc\\Device Parameters]"));
        // ... but not a line break.
        assert!(!reg.contains("BREAK"));
        assert!(reg.contains("; Record #3 was left out"));
        assert!(!assets.readme.contains("LINE\r\nBREAK"));
    }

    #[test]
    fn other_values_and_strings() {
        let keychron = fixtures::keychron().instance_id;
        let baselines = vec![
            baseline(
                device(&keychron),
                HID_TYPE,
                RegValue::Other {
                    reg_type: 3,
                    data_hex: "0a0bff".into(),
                },
            ),
            baseline(
                device(&keychron),
                HID_SUBTYPE,
                RegValue::Other {
                    reg_type: 1,
                    data_hex: "340000".into(),
                },
            ),
            baseline(
                WriteTarget::Global,
                LAYER_DRIVER_JPN,
                RegValue::Sz {
                    value: r"odd\".into(),
                },
            ),
            baseline(
                WriteTarget::Global,
                KEYBOARD_IDENTIFIER,
                RegValue::Other {
                    reg_type: 3,
                    data_hex: "abc".into(),
                },
            ),
            baseline(WriteTarget::Global, "Start", dword(1)),
        ];
        let assets = render_recovery_assets(&baselines, "0.1.0", GENERATED_AT).unwrap();
        let reg = reg_text(&assets);
        assert!(reg.contains("\"KeyboardTypeOverride\"=hex:0a,0b,ff\r\n"));
        assert!(reg.contains("\"KeyboardSubtypeOverride\"=hex(1):34,00,00\r\n"));
        assert!(reg.contains("\"LayerDriver JPN\"=\"odd\\\\\"\r\n"));
        assert!(reg.contains("; Record #4 was left out"));
        assert!(reg.contains("; Record #5 was left out: MKLM does not write that value."));
        assert!(!reg.contains("Start"));
        assert!(assets.cmd.contains("/t REG_BINARY /d 0a0bff /f"));
        assert!(assets.cmd.contains("rem Record #2 has registry type 1"));
        assert_eq!(assets.skipped, vec![1, 2, 3, 4]);
    }

    #[test]
    fn nothing_to_render() {
        assert_eq!(
            render_recovery_assets(&[], "0.1.0", GENERATED_AT),
            Err(AssetError::Empty)
        );
    }

    #[test]
    fn readme_starts_with_undo() {
        let assets = render_recovery_assets(&dev_machine(), "0.1.0", GENERATED_AT).unwrap();
        assert!(assets.readme.starts_with('\u{feff}'));
        let undo = assets.readme.find("mklm-cli undo").unwrap();
        assert!(undo < assets.readme.find("mklm-cli recover").unwrap());
        assert!(undo < assets.readme.find(RESTORE_CMD_FILE).unwrap());
        assert!(assets.readme.contains("2026-09-27 04:00 UTC"));
        assert!(assets.readme.contains("MKLM 0.1.0"));
        assert!(
            assets
                .readme
                .contains(r"ProgramData\SHIN DATA CENTER\MKLM\Recovery")
        );
        // A version string that is not plain text never reaches the script (it is not quoted there).
        for odd in ["1.0 & calc", "1.0%PATH%", "", "1.0\r\ncalc"] {
            let assets = render_recovery_assets(&dev_machine(), odd, GENERATED_AT).unwrap();
            assert!(assets.cmd.contains("by MKLM (unknown version)."), "{odd:?}");
            assert!(!assets.cmd.contains("calc") && !assets.cmd.contains("PATH"));
        }
        let beta = render_recovery_assets(&dev_machine(), "0.2.0-beta_1", GENERATED_AT).unwrap();
        assert!(beta.cmd.contains("by MKLM 0.2.0-beta_1."));
    }

    #[test]
    fn utc_dates() {
        assert_eq!(format_utc(Timestamp(0)), "1970-01-01 00:00 UTC");
        assert_eq!(format_utc(GENERATED_AT), "2026-09-27 04:00 UTC");
        assert_eq!(
            format_utc(Timestamp(1_790_500_000_000)),
            "2026-09-27 09:06 UTC"
        );
        // 2024-02-29 23:59:59.999
        assert_eq!(
            format_utc(Timestamp(1_709_251_199_999)),
            "2024-02-29 23:59 UTC"
        );
        assert_eq!(
            format_utc(Timestamp(951_782_400_000)),
            "2000-02-29 00:00 UTC"
        );
        assert_eq!(
            format_utc(Timestamp(4_102_444_800_000)),
            "2100-01-01 00:00 UTC"
        );
    }
}
