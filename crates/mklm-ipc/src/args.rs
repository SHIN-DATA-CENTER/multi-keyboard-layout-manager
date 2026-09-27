//! The helper's command line (plan 2.2): only the pipe name, a nonce and the caller's PID, in one
//! fixed format that [`HelperArgs::parse`] validates as strictly as [`HELPER_ARGS_PATTERN`].

use std::fmt;

/// Prefix of every pipe path.
pub const PIPE_PATH_PREFIX: &str = r"\\.\pipe\";
/// Prefix of MKLM's helper pipe names; a lower-case hyphenated UUID follows.
pub const PIPE_NAME_PREFIX: &str = "SHINDATACENTER.MKLM.";
/// Length of the nonce in bytes (64 hex digits on the command line).
pub const NONCE_LEN: usize = 32;

/// The exact grammar of the helper's arguments (everything after the program name), anchored.
/// [`HelperArgs::parse`] implements it by hand (no regex dependency) and its tests check both.
pub const HELPER_ARGS_PATTERN: &str = r"^--pipe SHINDATACENTER\.MKLM\.[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12} --nonce [0-9a-f]{64} --caller-pid [1-9][0-9]{0,9}$";

/// Length of a hyphenated UUID.
const UUID_LEN: usize = 36;
/// Byte offsets of the hyphens in a hyphenated UUID.
const UUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];
/// Most digits a PID may have on the command line (`u32::MAX` has ten).
const MAX_PID_DIGITS: usize = 10;

const PIPE_FLAG: &str = "--pipe ";
const NONCE_FLAG: &str = " --nonce ";
const CALLER_PID_FLAG: &str = " --caller-pid ";

/// A pipe name `SHINDATACENTER.MKLM.<uuid>` (without the `\\.\pipe\` prefix).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipeName(String);

impl PipeName {
    /// Builds the name from a freshly generated UUID (lower-case, hyphenated).
    pub fn from_uuid(uuid: &str) -> Result<Self, ArgsError> {
        if is_lower_uuid(uuid) {
            Ok(Self(format!("{PIPE_NAME_PREFIX}{uuid}")))
        } else {
            Err(ArgsError::Malformed)
        }
    }

    /// Validates a full name.
    pub fn parse(name: &str) -> Result<Self, ArgsError> {
        let uuid = name
            .strip_prefix(PIPE_NAME_PREFIX)
            .ok_or(ArgsError::Malformed)?;
        Self::from_uuid(uuid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `\\.\pipe\SHINDATACENTER.MKLM.<uuid>`.
    pub fn path(&self) -> String {
        format!("{PIPE_PATH_PREFIX}{}", self.0)
    }
}

/// Random bytes that bind one helper launch to one caller. `Debug` never prints them.
#[derive(Clone, PartialEq, Eq)]
pub struct Nonce([u8; NONCE_LEN]);

impl Nonce {
    pub fn from_bytes(bytes: [u8; NONCE_LEN]) -> Self {
        Self(bytes)
    }

    /// Parses exactly 64 lower-case hex digits.
    pub fn parse_hex(text: &str) -> Result<Self, ArgsError> {
        let digits = text.as_bytes();
        if digits.len() != NONCE_LEN * 2 {
            return Err(ArgsError::Malformed);
        }
        let (pairs, _) = digits.as_chunks::<2>();
        let mut bytes = [0u8; NONCE_LEN];
        for (byte, &[high, low]) in bytes.iter_mut().zip(pairs) {
            let high = lower_hex_value(high).ok_or(ArgsError::Malformed)?;
            let low = lower_hex_value(low).ok_or(ArgsError::Malformed)?;
            *byte = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    /// 64 lower-case hex digits.
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(NONCE_LEN * 2);
        for byte in self.0 {
            text.push(char::from(DIGITS[usize::from(byte >> 4)]));
            text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        text
    }

    /// Constant-time comparison.
    pub fn matches(&self, other: &Nonce) -> bool {
        // Accumulate every difference instead of returning at the first one, so that the time
        // taken does not tell how many leading bytes matched.
        let mut difference = 0u8;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            difference |= a ^ b;
        }
        std::hint::black_box(difference) == 0
    }
}

impl fmt::Debug for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Nonce(..)")
    }
}

/// The helper's parsed arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperArgs {
    pub pipe: PipeName,
    pub nonce: Nonce,
    /// PID of the caller that created the pipe; the helper checks it against
    /// `GetNamedPipeServerProcessId`.
    pub caller_pid: u32,
}

impl HelperArgs {
    /// Parses the arguments part of the raw command line (see [`command_line_tail`]) and accepts
    /// nothing but [`HELPER_ARGS_PATTERN`]: single spaces, this order, no quotes, no extra
    /// arguments, a PID in 1..=u32::MAX.
    pub fn parse(tail: &str) -> Result<Self, ArgsError> {
        let rest = tail.strip_prefix(PIPE_FLAG).ok_or(ArgsError::Malformed)?;
        let (pipe, rest) = rest
            .split_at_checked(PIPE_NAME_PREFIX.len() + UUID_LEN)
            .ok_or(ArgsError::Malformed)?;
        let pipe = PipeName::parse(pipe)?;

        let rest = rest.strip_prefix(NONCE_FLAG).ok_or(ArgsError::Malformed)?;
        let (nonce, rest) = rest
            .split_at_checked(NONCE_LEN * 2)
            .ok_or(ArgsError::Malformed)?;
        let nonce = Nonce::parse_hex(nonce)?;

        let pid = rest
            .strip_prefix(CALLER_PID_FLAG)
            .ok_or(ArgsError::Malformed)?;
        let caller_pid = parse_pid(pid)?;

        Ok(Self {
            pipe,
            nonce,
            caller_pid,
        })
    }

    /// The `lpParameters` string for `ShellExecuteExW`; `parse` accepts exactly this (for a
    /// non-zero `caller_pid`, which every real process has).
    pub fn to_parameters(&self) -> String {
        format!(
            "{PIPE_FLAG}{}{NONCE_FLAG}{}{CALLER_PID_FLAG}{}",
            self.pipe.as_str(),
            self.nonce.to_hex(),
            self.caller_pid
        )
    }
}

/// Strips the program name from a raw `GetCommandLineW` string (quoted or unquoted, as the CRT
/// parses argv\[0\]) and the single space after it. `None` when nothing follows.
///
/// As in the UCRT, a `"` anywhere in the program name toggles quoting (the quotes are not part of
/// the name) and an unquoted space or tab ends it. Only one space is removed: any further
/// whitespace stays in the result, where [`HelperArgs::parse`] rejects it. An empty program name
/// (the line starts with whitespace, or with `""`) and an unterminated quote give `None`.
pub fn command_line_tail(command_line: &str) -> Option<&str> {
    let mut in_quotes = false;
    let mut name_chars = 0usize;
    let mut end = command_line.len();
    for (index, c) in command_line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                end = index;
                break;
            }
            _ => name_chars += 1,
        }
    }
    if name_chars == 0 || in_quotes {
        return None;
    }
    let rest = &command_line[end..];
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    (!rest.is_empty()).then_some(rest)
}

/// The helper's command line is not in the fixed format. Deliberately says nothing about which
/// part failed: the helper logs it and exits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    #[error("the helper's command line is malformed")]
    Malformed,
}

/// `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`.
fn is_lower_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == UUID_LEN
        && bytes.iter().enumerate().all(|(index, &byte)| {
            if UUID_HYPHENS.contains(&index) {
                byte == b'-'
            } else {
                lower_hex_value(byte).is_some()
            }
        })
}

/// `[0-9a-f]` → its value. Upper-case digits are rejected on purpose: the format has one spelling.
fn lower_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// `[1-9][0-9]{0,9}` that fits in a `u32`. ASCII digits only: no sign, no leading zero, no other
/// Unicode digits, nothing after the number.
fn parse_pid(text: &str) -> Result<u32, ArgsError> {
    let digits = text.as_bytes();
    let well_formed = !digits.is_empty()
        && digits.len() <= MAX_PID_DIGITS
        && digits[0] != b'0'
        && digits.iter().all(u8::is_ascii_digit);
    if !well_formed {
        return Err(ArgsError::Malformed);
    }
    let value = digits
        .iter()
        .fold(0u64, |value, &digit| value * 10 + u64::from(digit - b'0'));
    u32::try_from(value).map_err(|_| ArgsError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f5";
    const NONCE_HEX: &str = "00112233445566778899aabbccddeeff0123456789abcdeffedcba9876543210";

    fn valid_tail() -> String {
        format!("--pipe SHINDATACENTER.MKLM.{UUID} --nonce {NONCE_HEX} --caller-pid 4242")
    }

    fn sample_args() -> HelperArgs {
        HelperArgs {
            pipe: PipeName::from_uuid(UUID).unwrap(),
            nonce: Nonce::parse_hex(NONCE_HEX).unwrap(),
            caller_pid: 4242,
        }
    }

    #[test]
    fn parses_the_fixed_format() {
        let args = HelperArgs::parse(&valid_tail()).unwrap();
        assert_eq!(args, sample_args());
        assert_eq!(args.pipe.as_str(), format!("SHINDATACENTER.MKLM.{UUID}"));
        assert_eq!(
            args.pipe.path(),
            format!(r"\\.\pipe\SHINDATACENTER.MKLM.{UUID}")
        );
        assert_eq!(args.nonce.to_hex(), NONCE_HEX);
        assert_eq!(args.caller_pid, 4242);
    }

    #[test]
    fn to_parameters_is_what_parse_accepts() {
        let args = sample_args();
        assert_eq!(args.to_parameters(), valid_tail());
        assert_eq!(HelperArgs::parse(&args.to_parameters()).unwrap(), args);

        for caller_pid in [1, 9, 10, 4242, 999_999_999, 1_000_000_000, u32::MAX] {
            let args = HelperArgs {
                caller_pid,
                ..sample_args()
            };
            assert_eq!(HelperArgs::parse(&args.to_parameters()).unwrap(), args);
        }
    }

    #[test]
    fn accepts_the_whole_pid_range() {
        let with_pid = |pid: &str| HelperArgs::parse(&valid_tail().replace("4242", pid));
        assert_eq!(with_pid("1").unwrap().caller_pid, 1);
        assert_eq!(with_pid("4294967295").unwrap().caller_pid, u32::MAX);
        assert_eq!(with_pid("1000000000").unwrap().caller_pid, 1_000_000_000);
    }

    /// The cases section H.1 lists, and a few more.
    #[test]
    fn rejects_everything_else() {
        let valid = valid_tail();
        let pipe = format!("SHINDATACENTER.MKLM.{UUID}");
        let rejected = [
            // Two spaces, at each separator.
            valid.replacen(' ', "  ", 1),
            valid.replace(" --nonce", "  --nonce"),
            valid.replace(" --caller-pid", "  --caller-pid"),
            valid.replace("--caller-pid ", "--caller-pid  "),
            // Tabs instead of spaces.
            valid.replace(" --nonce", "\t--nonce"),
            valid.replacen(' ', "\t", 1),
            // Upper-case hex.
            valid.replace(UUID, &UUID.to_uppercase()),
            valid.replace(NONCE_HEX, &NONCE_HEX.to_uppercase()),
            valid.replace("aabb", "AAbb"),
            // Extra arguments.
            format!("{valid} --extra"),
            format!("{valid} "),
            format!("--uninstall-restore {valid}"),
            format!("{valid}\n"),
            format!("{valid}\r\n"),
            format!(" {valid}"),
            // Quotes.
            valid.replace(&pipe, &format!("\"{pipe}\"")),
            valid.replace(NONCE_HEX, &format!("\"{NONCE_HEX}\"")),
            valid.replace("4242", "\"4242\""),
            format!("\"{valid}\""),
            // PID 0, too large, leading zero, signs, other digits.
            valid.replace("4242", "0"),
            valid.replace("4242", "00"),
            valid.replace("4242", "04242"),
            valid.replace("4242", "4294967296"),
            valid.replace("4242", "9999999999"),
            valid.replace("4242", "12345678901"),
            valid.replace("4242", "+4242"),
            valid.replace("4242", "-4242"),
            valid.replace("4242", "4242 "),
            valid.replace("4242", ""),
            valid.replace("4242", "４２４２"),
            valid.replace("4242", "٤٢"),
            valid.replace("4242", "42²"),
            valid.replace("4242", "0x10"),
            // Wrong lengths or shapes.
            valid.replace(NONCE_HEX, &NONCE_HEX[..62]),
            valid.replace(NONCE_HEX, &format!("{NONCE_HEX}00")),
            valid.replace(UUID, &format!("{{{UUID}}}")),
            valid.replace(UUID, &UUID.replace('-', "")),
            valid.replace(UUID, "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f"),
            valid.replace(UUID, "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4fg"),
            valid.replace(UUID, "0f8c2d4e_5b6a-4c3d-9e8f-a0b1c2d3e4f5"),
            valid.replace("SHINDATACENTER.MKLM.", "SHINDATACENTER.MKLM_"),
            valid.replace("SHINDATACENTER.MKLM.", "shindatacenter.mklm."),
            valid.replace(&pipe, &format!(r"\\.\pipe\{pipe}")),
            // Flags: other order, other spelling, missing.
            format!("--nonce {NONCE_HEX} --pipe {pipe} --caller-pid 4242"),
            valid.replace("--pipe", "--Pipe"),
            valid.replace("--pipe", "-pipe"),
            valid.replace("--pipe", "/pipe"),
            valid.replace("--nonce", "--nonce="),
            valid.replace("--caller-pid", "--caller_pid"),
            format!("--pipe {pipe} --nonce {NONCE_HEX}"),
            format!("--pipe {pipe}"),
            String::new(),
        ];
        for tail in &rejected {
            assert_eq!(
                HelperArgs::parse(tail),
                Err(ArgsError::Malformed),
                "accepted {tail:?}"
            );
        }
    }

    #[test]
    fn pipe_names() {
        let name = PipeName::from_uuid(UUID).unwrap();
        assert_eq!(PipeName::parse(name.as_str()).unwrap(), name);
        for uuid in [
            "",
            "0F8C2D4E-5B6A-4C3D-9E8F-A0B1C2D3E4F5",
            "{0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f5}",
            "0f8c2d4e5b6a4c3d9e8fa0b1c2d3e4f5",
            "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f5 ",
            "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f5\\x",
            "0f8c2d4e-5b6a-4c3d-9e8fa-0b1c2d3e4f5",
        ] {
            assert_eq!(
                PipeName::from_uuid(uuid),
                Err(ArgsError::Malformed),
                "{uuid}"
            );
        }
        for name in [
            UUID.to_string(),
            format!("SHINDATACENTER.MKLM.{UUID}.x"),
            format!("SHINDATACENTER.MKLM.MKLM.{UUID}"),
            format!(r"\\.\pipe\SHINDATACENTER.MKLM.{UUID}"),
            format!("Other.{UUID}"),
        ] {
            assert_eq!(PipeName::parse(&name), Err(ArgsError::Malformed), "{name}");
        }
    }

    #[test]
    fn nonces() {
        let nonce = Nonce::parse_hex(NONCE_HEX).unwrap();
        assert_eq!(nonce.to_hex(), NONCE_HEX);
        let mut bytes = [0u8; NONCE_LEN];
        bytes[0] = 0x0a;
        bytes[NONCE_LEN - 1] = 0xf0;
        let from_bytes = Nonce::from_bytes(bytes);
        assert_eq!(
            from_bytes.to_hex(),
            format!("0a{}f0", "0".repeat(NONCE_LEN * 2 - 4))
        );
        assert_eq!(Nonce::parse_hex(&from_bytes.to_hex()).unwrap(), from_bytes);

        assert!(nonce.matches(&nonce.clone()));
        for index in 0..NONCE_LEN {
            let mut other = nonce.0;
            other[index] ^= 0x01;
            assert!(!nonce.matches(&Nonce::from_bytes(other)), "byte {index}");
        }

        for text in [
            "",
            &NONCE_HEX[..63],
            &format!("{NONCE_HEX}0"),
            &NONCE_HEX.to_uppercase(),
            &NONCE_HEX.replace('a', "g"),
            &NONCE_HEX.replacen("00", "0 ", 1),
            &NONCE_HEX.replacen("00", "+0", 1),
            &NONCE_HEX.replacen("00", "é", 1),
        ] {
            assert_eq!(Nonce::parse_hex(text), Err(ArgsError::Malformed), "{text}");
        }
    }

    #[test]
    fn debug_never_prints_the_nonce() {
        let args = sample_args();
        let text = format!("{args:?} {:?}", args.nonce);
        assert!(!text.contains(NONCE_HEX), "{text}");
        assert!(!text.contains("0, 17, 34"), "{text}");
        assert!(text.contains("Nonce(..)"), "{text}");
    }

    #[test]
    fn command_line_tails() {
        let tail = valid_tail();
        let cases: [(String, Option<&str>); 17] = [
            // Quoted program name with spaces (how the launcher starts the helper).
            (
                format!(r#""C:\Program Files\MKLM\mklm-helper.exe" {tail}"#),
                Some(&tail),
            ),
            // Unquoted.
            (format!(r"C:\MKLM\mklm-helper.exe {tail}"), Some(&tail)),
            (format!("mklm-helper {tail}"), Some(&tail)),
            // Quotes in the middle toggle quoting, as in the UCRT.
            (
                format!(r#"C:\"Program Files"\MKLM\mklm-helper.exe {tail}"#),
                Some(&tail),
            ),
            // Only one space is stripped; the rest is left for `parse` to reject.
            (format!("helper.exe  {tail}"), Some(" --pipe")),
            (format!("helper.exe\t{tail}"), Some("\t--pipe")),
            (format!("\"helper.exe\"\t{tail}"), Some("\t--pipe")),
            // Nothing follows.
            ("helper.exe".to_string(), None),
            ("helper.exe ".to_string(), None),
            (
                r#""C:\Program Files\MKLM\mklm-helper.exe""#.to_string(),
                None,
            ),
            (
                r#""C:\Program Files\MKLM\mklm-helper.exe" "#.to_string(),
                None,
            ),
            (String::new(), None),
            // No program name.
            (format!(" {tail}"), None),
            (format!("\t{tail}"), None),
            (format!(r#""" {tail}"#), None),
            // Unterminated quote: everything is the program name.
            (
                format!(r#""C:\Program Files\MKLM\mklm-helper.exe {tail}"#),
                None,
            ),
            // Quote right after the name keeps going until the next unquoted space.
            (format!(r#""C:\x\helper.exe"x {tail}"#), Some(tail.as_str())),
        ];
        for (line, expected) in &cases {
            let got = command_line_tail(line);
            match expected {
                Some(expected) if expected.len() < tail.len() => {
                    // Prefix check for the whitespace cases.
                    assert!(
                        got.is_some_and(|got| got.starts_with(expected) && got.ends_with("4242")),
                        "{line:?} → {got:?}"
                    );
                    assert!(got.is_some_and(|got| HelperArgs::parse(got).is_err()));
                }
                Some(expected) => assert_eq!(got, Some(*expected), "{line:?}"),
                None => assert_eq!(got, None, "{line:?}"),
            }
        }
    }

    #[test]
    fn full_command_line_round_trip() {
        let args = sample_args();
        let line = format!(
            r#""C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe" {}"#,
            args.to_parameters()
        );
        let tail = command_line_tail(&line).unwrap();
        assert_eq!(HelperArgs::parse(tail).unwrap(), args);
    }

    // ---- The hand-written parser against the pattern itself ----

    /// A matcher for the regex subset [`HELPER_ARGS_PATTERN`] uses (`^`, `$`, literals, `\.`,
    /// bracket classes with ranges, `{n}` and `{m,n}`), written independently of the parser so
    /// that the differential test below checks the parser against the pattern's text.
    mod mini_regex {
        #[derive(Debug)]
        enum Atom {
            Literal(char),
            Class(Vec<(char, char)>),
        }

        impl Atom {
            fn matches(&self, c: char) -> bool {
                match self {
                    Atom::Literal(literal) => *literal == c,
                    Atom::Class(ranges) => ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi),
                }
            }
        }

        #[derive(Debug)]
        struct Token {
            atom: Atom,
            min: usize,
            max: usize,
        }

        fn number(chars: &[char], at: &mut usize) -> usize {
            let start = *at;
            while chars[*at].is_ascii_digit() {
                *at += 1;
            }
            chars[start..*at]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap()
        }

        fn compile(pattern: &str) -> Vec<Token> {
            let chars: Vec<char> = pattern.chars().collect();
            assert_eq!(chars.first(), Some(&'^'), "pattern must be anchored");
            assert_eq!(chars.last(), Some(&'$'), "pattern must be anchored");
            let chars = &chars[1..chars.len() - 1];
            let mut tokens = Vec::new();
            let mut at = 0;
            while at < chars.len() {
                let atom = match chars[at] {
                    '\\' => {
                        at += 2;
                        Atom::Literal(chars[at - 1])
                    }
                    '[' => {
                        at += 1;
                        let mut ranges = Vec::new();
                        while chars[at] != ']' {
                            if chars[at + 1] == '-' && chars[at + 2] != ']' {
                                ranges.push((chars[at], chars[at + 2]));
                                at += 3;
                            } else {
                                ranges.push((chars[at], chars[at]));
                                at += 1;
                            }
                        }
                        at += 1;
                        Atom::Class(ranges)
                    }
                    c => {
                        assert!(!"(){}*+?|.$^".contains(c), "unsupported {c:?}");
                        at += 1;
                        Atom::Literal(c)
                    }
                };
                let (min, max) = if chars.get(at) == Some(&'{') {
                    at += 1;
                    let min = number(chars, &mut at);
                    let max = if chars[at] == ',' {
                        at += 1;
                        number(chars, &mut at)
                    } else {
                        min
                    };
                    assert_eq!(chars[at], '}');
                    at += 1;
                    (min, max)
                } else {
                    (1, 1)
                };
                tokens.push(Token { atom, min, max });
            }
            tokens
        }

        fn match_here(tokens: &[Token], text: &[char]) -> bool {
            let Some((token, rest)) = tokens.split_first() else {
                return text.is_empty();
            };
            let mut run = 0;
            while run < token.max && run < text.len() && token.atom.matches(text[run]) {
                run += 1;
            }
            run >= token.min
                && (token.min..=run)
                    .rev()
                    .any(|n| match_here(rest, &text[n..]))
        }

        /// Whole-input match (`$` is the end of the input, as in the `regex` crate).
        pub fn is_match(pattern: &str, text: &str) -> bool {
            let text: Vec<char> = text.chars().collect();
            match_here(&compile(pattern), &text)
        }
    }

    /// What `parse` must accept: the pattern, plus "the PID fits in a u32" (section E.4).
    fn expected_accept(text: &str) -> bool {
        mini_regex::is_match(HELPER_ARGS_PATTERN, text)
            && text
                .rsplit(' ')
                .next()
                .and_then(|pid| pid.parse::<u64>().ok())
                .is_some_and(|pid| pid <= u64::from(u32::MAX))
    }

    #[test]
    fn mini_regex_sanity() {
        assert!(mini_regex::is_match(HELPER_ARGS_PATTERN, &valid_tail()));
        assert!(!mini_regex::is_match(
            HELPER_ARGS_PATTERN,
            &format!("{} ", valid_tail())
        ));
        assert!(mini_regex::is_match(r"^a[0-9]{1,3}\.b$", "a12.b"));
        assert!(!mini_regex::is_match(r"^a[0-9]{1,3}\.b$", "a1234.b"));
        assert!(!mini_regex::is_match(r"^a[0-9]{1,3}\.b$", "a12xb"));
    }

    /// Every single-character deletion, substitution and insertion of a valid tail, and a range of
    /// PIDs: `parse` accepts exactly what the pattern (plus the u32 bound) accepts.
    #[test]
    fn parse_agrees_with_the_pattern() {
        let alphabet = [
            ' ', '\t', '\n', '\r', '"', '\'', '-', '.', '_', '/', '\\', '=', '+', '0', '1', '5',
            '9', 'a', 'f', 'g', 'z', 'A', 'F', 'M', 'é', '１', '٣', '\u{0}', '\u{a0}',
        ];
        let valid = valid_tail();
        let chars: Vec<char> = valid.chars().collect();
        let mut inputs = vec![valid.clone()];
        for index in 0..=chars.len() {
            for &c in &alphabet {
                let mut inserted = chars.clone();
                inserted.insert(index, c);
                inputs.push(inserted.into_iter().collect());
                if index < chars.len() {
                    let mut replaced = chars.clone();
                    replaced[index] = c;
                    inputs.push(replaced.into_iter().collect());
                }
            }
            if index < chars.len() {
                let mut deleted = chars.clone();
                deleted.remove(index);
                inputs.push(deleted.into_iter().collect());
            }
        }
        for pid in [
            "0",
            "1",
            "01",
            "10",
            "429496729",
            "4294967294",
            "4294967295",
            "4294967296",
            "4294967300",
            "5000000000",
            "9999999999",
            "10000000000",
            "18446744073709551616",
        ] {
            inputs.push(valid.replace("4242", pid));
        }

        let mut accepted = 0;
        for input in &inputs {
            let expected = expected_accept(input);
            let parsed = HelperArgs::parse(input);
            assert_eq!(parsed.is_ok(), expected, "{input:?}");
            if let Ok(args) = parsed {
                assert_eq!(&args.to_parameters(), input);
                accepted += 1;
            }
        }
        // Substituting a hex digit by another keeps the tail valid, so plenty are accepted.
        assert!(accepted > 100, "{accepted}");
    }
}
