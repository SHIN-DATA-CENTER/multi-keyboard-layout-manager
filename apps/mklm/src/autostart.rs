//! Start in the notification area at sign-in (plan 3.9; design m3 F.3): the GUI's own HKCU Run
//! value `SHINDATACENTER.MKLM` = `"<dir>\mklm.exe" --tray`, written through
//! `mklm_win::session` (the only HKCU writer). On by default: the wizard's last step turns it on.
//!
//! Rules (WP-U6):
//! - The setting shown is the value itself (`autostart_state`), not a copy in settings.toml.
//! - At start, when the value exists but names another path (MKLM moved), rewrite it; when Task
//!   Manager disabled it (`disabled_by_user`), never turn it back on — show "Windows のスタート
//!   アップ設定で無効になっています" next to the switch instead.
//! - Turning it off removes the value; M5's uninstaller removes it too.

use mklm_win::session::AutostartState;

/// The command line to register for `exe`.
pub fn command_line(exe: &std::path::Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// Whether the registered value must be rewritten for `expected` (it names another path).
pub fn needs_repair(state: &AutostartState, expected: &str) -> bool {
    state
        .command_line
        .as_deref()
        .is_some_and(|registered| !registered.eq_ignore_ascii_case(expected))
        && !state.disabled_by_user
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_rules() {
        let expected = command_line(std::path::Path::new(r"C:\Program Files\MKLM\mklm.exe"));
        assert_eq!(expected, r#""C:\Program Files\MKLM\mklm.exe" --tray"#);
        let moved = AutostartState {
            command_line: Some(r#""D:\old\mklm.exe" --tray"#.into()),
            disabled_by_user: false,
        };
        assert!(needs_repair(&moved, &expected));
        let disabled = AutostartState {
            disabled_by_user: true,
            ..moved.clone()
        };
        assert!(!needs_repair(&disabled, &expected));
        assert!(!needs_repair(&AutostartState::default(), &expected));
    }
}
