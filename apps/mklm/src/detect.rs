//! Layout detection (plan 3.3; design m3 B.3, B.4): which layout a keyboard physically has, from
//! the scan codes of two keys. Scan codes are physical (set 1 make codes from Raw Input, via
//! winit's `PhysicalKeyExtScancode`), so neither the stored override nor the IME changes them.
//!
//! | Question | JIS | US |
//! |---|---|---|
//! | The key left of Backspace | 0x7D (¥) | 0x0D (=) |
//! | The key left of right Shift | 0x73 (ろ) | 0x35 (/) |
//!
//! Detection runs inside the change page, for the keyboard being changed
//! ([`Detection::for_keyboards`], its instance IDs): presses from other keyboards are reported
//! as [`Press::OtherKeyboard`] ("内蔵キーボードのキーです。Keychron Receiver で押してください")
//! and never counted. Without targets (the wizard's rows), the first **answer** key fixes the
//! keyboard — a stray Tab or letter does not. Keys only JIS keyboards have (0x70 かな, 0x79 変換,
//! 0x7B 無変換, 0x7D, 0x73) make "JIS is likely" a hint.

/// Scan codes used by the detection.
pub mod scancode {
    pub const YEN: u32 = 0x7D;
    pub const EQUAL: u32 = 0x0D;
    pub const RO: u32 = 0x73;
    pub const SLASH: u32 = 0x35;
    pub const KANA: u32 = 0x70;
    pub const CONVERT: u32 = 0x79;
    pub const NON_CONVERT: u32 = 0x7B;
}

/// True for a key only JIS keyboards have (design m3 B.2: the physical layout is learned from
/// it, JIS only).
pub fn is_jis_only_key(code: u32) -> bool {
    matches!(
        code,
        scancode::KANA | scancode::CONVERT | scancode::NON_CONVERT | scancode::YEN | scancode::RO
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    LeftOfBackspace,
    LeftOfRightShift,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Jis,
    Us,
    /// The two answers disagree (or a key was mistaken): start again.
    Mixed,
}

/// What one key press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// It answered the current question.
    Counted,
    /// A key of the right keyboard that answers nothing (the page says which key to press).
    NotAnAnswer,
    /// A key of another keyboard: not counted; the page names the keyboard to use.
    OtherKeyboard,
    /// Both questions are answered; "やり直す" starts again.
    Finished,
}

/// The detection's state for one keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub step: Step,
    /// The keyboards (instance IDs of one physical device) whose keys count; empty: any, until
    /// the first answer key fixes [`Self::device`].
    targets: Vec<String>,
    /// The keyboard (instance ID) the answers came from.
    pub device: Option<String>,
    answers: Vec<bool>,
    /// A JIS-only key was pressed on the keyboard.
    pub jis_hint: bool,
}

impl Default for Detection {
    fn default() -> Self {
        Self::new()
    }
}

impl Detection {
    /// Any keyboard; the first answer key fixes it.
    pub fn new() -> Self {
        Self::for_keyboards(Vec::new())
    }

    /// Only the keys of `targets` (the members of the row being changed) count.
    pub fn for_keyboards(targets: Vec<String>) -> Self {
        Self {
            step: Step::LeftOfBackspace,
            targets,
            device: None,
            answers: Vec::new(),
            jis_hint: false,
        }
    }

    /// "やり直す": the same targets, no answers.
    pub fn restart(&mut self) {
        *self = Self::for_keyboards(std::mem::take(&mut self.targets));
    }

    fn accepts(&self, device: &str) -> bool {
        let same = |id: &String| id.eq_ignore_ascii_case(device);
        match &self.device {
            Some(fixed) => same(fixed),
            None => self.targets.is_empty() || self.targets.iter().any(same),
        }
    }

    /// Takes one key press.
    pub fn press(&mut self, device: &str, code: u32) -> Press {
        if !self.accepts(device) {
            return Press::OtherKeyboard;
        }
        if self.step == Step::Done {
            return Press::Finished;
        }
        let answer = match (self.step, code) {
            (Step::LeftOfBackspace, scancode::YEN) => Some(true),
            (Step::LeftOfBackspace, scancode::EQUAL) => Some(false),
            (Step::LeftOfRightShift, scancode::RO) => Some(true),
            (Step::LeftOfRightShift, scancode::SLASH) => Some(false),
            _ => None,
        };
        let fixed_or_target = self.device.is_some() || !self.targets.is_empty();
        if is_jis_only_key(code) && (fixed_or_target || answer.is_some()) {
            self.jis_hint = true;
        }
        let Some(jis) = answer else {
            return Press::NotAnAnswer;
        };
        if self.device.is_none() {
            self.device = Some(device.to_string());
        }
        self.answers.push(jis);
        self.step = match self.step {
            Step::LeftOfBackspace => Step::LeftOfRightShift,
            Step::LeftOfRightShift | Step::Done => Step::Done,
        };
        Press::Counted
    }

    /// The verdict once both keys were answered.
    pub fn verdict(&self) -> Option<Verdict> {
        match self.answers.as_slice() {
            [true, true] => Some(Verdict::Jis),
            [false, false] => Some(Verdict::Us),
            [_, _] => Some(Verdict::Mixed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

    #[test]
    fn a_us_keyboard() {
        let mut detection = Detection::new();
        // A stray key does not fix the keyboard (review U14).
        assert_eq!(detection.press(BUILT_IN, 0x0F), Press::NotAnAnswer); // Tab
        assert_eq!(detection.device, None);
        assert_eq!(detection.press(KEYCHRON, scancode::EQUAL), Press::Counted);
        assert_eq!(
            detection.press(BUILT_IN, scancode::RO),
            Press::OtherKeyboard
        );
        assert_eq!(detection.press(KEYCHRON, scancode::SLASH), Press::Counted);
        assert_eq!(detection.verdict(), Some(Verdict::Us));
        assert_eq!(detection.press(KEYCHRON, scancode::SLASH), Press::Finished);
        assert!(!detection.jis_hint);
    }

    #[test]
    fn a_jis_keyboard_a_mix_up_and_a_restart() {
        let mut detection = Detection::for_keyboards(vec![BUILT_IN.into()]);
        assert_eq!(
            detection.press(KEYCHRON, scancode::YEN),
            Press::OtherKeyboard
        );
        detection.press(BUILT_IN, scancode::KANA);
        assert!(detection.jis_hint);
        detection.press(BUILT_IN, scancode::YEN);
        detection.press(BUILT_IN, scancode::RO);
        assert_eq!(detection.verdict(), Some(Verdict::Jis));

        let mut mixed = Detection::for_keyboards(vec![KEYCHRON.into()]);
        mixed.press(KEYCHRON, scancode::YEN);
        mixed.press(KEYCHRON, scancode::SLASH);
        assert_eq!(mixed.verdict(), Some(Verdict::Mixed));
        mixed.restart();
        assert_eq!(mixed.verdict(), None);
        assert_eq!(mixed.press(BUILT_IN, scancode::EQUAL), Press::OtherKeyboard);
        assert_eq!(mixed.press(KEYCHRON, scancode::EQUAL), Press::Counted);
    }
}
