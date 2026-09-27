//! Answers typed by the user (design F.3): standard input is read one line at a time on a thread
//! of its own and handed over through a channel, so that the relay can wait for the helper and for
//! the user at the same time. Tests give a [`ScriptedInput`] instead.

#[cfg(test)]
use std::collections::VecDeque;
use std::io::{self, BufRead as _, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

/// One thing read from the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A line without its line break, trimmed.
    Text(String),
    /// End of input (Ctrl+Z, a closed pipe, `< NUL`): no answer will ever come.
    Eof,
}

/// Where answers come from.
pub trait Input {
    /// The next line. `timeout`: `None` waits as long as it takes, `Some(ZERO)` only looks.
    /// Returns `None` when nothing arrived in time. After [`Line::Eof`], every call returns `Eof`.
    fn next_line(&mut self, timeout: Option<Duration>) -> Option<Line>;

    /// Drops lines typed before a keep-or-revert question appeared, so that an early Enter or a
    /// `y` typed ahead cannot answer a question the user has not seen (a typed-ahead `y` would
    /// keep a layout nobody tested). Only for a console: piped input is a script's answers.
    fn discard_typed_ahead(&mut self) {}
}

/// Standard input, read on a thread that starts with the first question.
#[derive(Debug, Default)]
pub struct StdinInput {
    lines: Option<Receiver<Line>>,
    eof: bool,
}

impl StdinInput {
    pub fn new() -> Self {
        Self::default()
    }

    fn lines(&mut self) -> &Receiver<Line> {
        self.lines.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel();
            // The thread blocks in `read_line` for as long as the user does not type; it ends
            // with the process.
            thread::spawn(move || {
                let stdin = io::stdin();
                let mut stdin = stdin.lock();
                loop {
                    let mut text = String::new();
                    let line = match stdin.read_line(&mut text) {
                        Ok(0) | Err(_) => Line::Eof,
                        Ok(_) => Line::Text(text.trim().to_string()),
                    };
                    let eof = line == Line::Eof;
                    if sender.send(line).is_err() || eof {
                        return;
                    }
                }
            });
            receiver
        })
    }
}

impl Input for StdinInput {
    fn next_line(&mut self, timeout: Option<Duration>) -> Option<Line> {
        if self.eof {
            return Some(Line::Eof);
        }
        let received = match timeout {
            None => self.lines().recv().map_err(|_| ()),
            Some(timeout) if timeout.is_zero() => match self.lines().try_recv() {
                Ok(line) => Ok(line),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err(()),
            },
            Some(timeout) => match self.lines().recv_timeout(timeout) {
                Ok(line) => Ok(line),
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => Err(()),
            },
        };
        let line = received.unwrap_or(Line::Eof);
        if line == Line::Eof {
            self.eof = true;
        }
        Some(line)
    }

    fn discard_typed_ahead(&mut self) {
        use std::io::IsTerminal as _;

        if self.eof || !io::stdin().is_terminal() {
            return;
        }
        let Some(lines) = &self.lines else {
            return;
        };
        loop {
            match lines.try_recv() {
                Ok(Line::Text(_)) => {}
                Ok(Line::Eof) | Err(TryRecvError::Disconnected) => {
                    self.eof = true;
                    return;
                }
                Err(TryRecvError::Empty) => return,
            }
        }
    }
}

/// Lines given in advance (tests): each call takes the next one;
/// once they run out, `Eof`.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct ScriptedInput {
    lines: VecDeque<Line>,
}

#[cfg(test)]
impl ScriptedInput {
    pub fn new<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            lines: lines
                .into_iter()
                .map(|line| Line::Text(line.to_string()))
                .collect(),
        }
    }
}

#[cfg(test)]
impl Input for ScriptedInput {
    fn next_line(&mut self, _timeout: Option<Duration>) -> Option<Line> {
        Some(self.lines.pop_front().unwrap_or(Line::Eof))
    }
}

/// How the user answered a `[y/N]` question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YesNo {
    Yes,
    /// `n`, `no`, or just Enter (the default).
    No,
    /// End of input: no answer.
    NoAnswer,
}

/// Classifies one line of a `[y/N]` answer; `None` for anything else (ask again).
pub fn yes_no(line: &str) -> Option<YesNo> {
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(YesNo::Yes),
        "" | "n" | "no" => Some(YesNo::No),
        _ => None,
    }
}

/// Asks `question [y/N]` until the answer is yes, no or end of input.
pub fn ask_yes_no(input: &mut dyn Input, out: &mut dyn Write, question: &str) -> io::Result<YesNo> {
    loop {
        write!(out, "{question} [y/N] ")?;
        out.flush()?;
        match input.next_line(None) {
            Some(Line::Text(text)) => match yes_no(&text) {
                Some(answer) => return Ok(answer),
                None => writeln!(out, "Please answer y or n.")?,
            },
            Some(Line::Eof) | None => {
                writeln!(out)?;
                return Ok(YesNo::NoAnswer);
            }
        }
    }
}

/// Asks for one line (e.g. `restart` before a reboot); `None` at end of input.
pub fn ask_line(
    input: &mut dyn Input,
    out: &mut dyn Write,
    question: &str,
) -> io::Result<Option<String>> {
    write!(out, "{question} ")?;
    out.flush()?;
    Ok(match input.next_line(None) {
        Some(Line::Text(text)) => Some(text),
        Some(Line::Eof) | None => {
            writeln!(out)?;
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yes_no_answers() {
        assert_eq!(yes_no("y"), Some(YesNo::Yes));
        assert_eq!(yes_no(" YES "), Some(YesNo::Yes));
        assert_eq!(yes_no(""), Some(YesNo::No));
        assert_eq!(yes_no("n"), Some(YesNo::No));
        assert_eq!(yes_no("No"), Some(YesNo::No));
        assert_eq!(yes_no("maybe"), None);
        assert_eq!(yes_no("ye"), None);
    }

    #[test]
    fn questions_repeat_until_answered() {
        let mut out = Vec::new();
        let mut input = ScriptedInput::new(["what", "y"]);
        assert_eq!(
            ask_yes_no(&mut input, &mut out, "Continue?").unwrap(),
            YesNo::Yes
        );
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches("Continue? [y/N]").count(), 2);
        assert!(text.contains("Please answer y or n."));

        let mut out = Vec::new();
        let mut input = ScriptedInput::new([]);
        assert_eq!(
            ask_yes_no(&mut input, &mut out, "Continue?").unwrap(),
            YesNo::NoAnswer
        );
        let mut input = ScriptedInput::new(["restart"]);
        assert_eq!(
            ask_line(&mut input, &mut out, "Type restart:").unwrap(),
            Some("restart".into())
        );
        assert_eq!(ask_line(&mut input, &mut out, "Again:").unwrap(), None);
    }
}
