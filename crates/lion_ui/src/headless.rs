//! The interface without a screen, for the tests (C98): the events come from the file
//! named by `LION_UI_EVENTS`, one per line, and the image of each frame can be written
//! to the file named by `LION_UI_SNAPSHOT` (PPM).
//!
//! ```text
//! click Charger        a click on the button, the box or the field with this text
//! click 40 12          a click at this point
//! type Léa             a text typed in the field that has the focus
//! key backspace        a key: backspace, delete, enter, tab, escape, left, right, home, end
//! resize 640 480       a new size of the window
//! wait                 the tasks given to `ui.when_done` finish
//! close                the window is closed; also at the end of the file
//! ```

use std::collections::VecDeque;

use crate::canvas::Canvas;
use crate::{Event, Key};

pub struct Headless {
    events: VecDeque<Event>,
    snapshot: Option<String>,
}

impl Headless {
    pub fn open() -> Result<Headless, String> {
        let events = match std::env::var("LION_UI_EVENTS") {
            Ok(path) => {
                let text = std::fs::read_to_string(&path).map_err(|error| {
                    format!("cannot read the events of the interface in `{path}`: {error}")
                })?;
                parse(&text)?
            }
            Err(_) => VecDeque::new(),
        };
        Ok(Headless { events, snapshot: std::env::var("LION_UI_SNAPSHOT").ok() })
    }

    pub fn present(&mut self, canvas: &Canvas) -> Result<(), String> {
        if let Some(path) = &self.snapshot {
            std::fs::write(path, canvas.to_ppm())
                .map_err(|error| format!("cannot write `{path}`: {error}"))?;
        }
        Ok(())
    }

    pub fn next_event(&mut self) -> Event {
        self.events.pop_front().unwrap_or(Event::Close)
    }
}

fn parse(text: &str) -> Result<VecDeque<Event>, String> {
    let mut events = VecDeque::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        let numbers: Vec<i64> = rest.split_whitespace().filter_map(|part| part.parse().ok()).collect();
        let event = match word {
            "click" if numbers.len() == 2 && rest.split_whitespace().count() == 2 => {
                Event::Click { x: numbers[0], y: numbers[1] }
            }
            "click" if !rest.is_empty() => Event::ClickText(rest.to_string()),
            "type" => Event::Type(rest.to_string()),
            "key" => Event::Key(match rest {
                "backspace" => Key::Backspace,
                "delete" => Key::Delete,
                "enter" => Key::Enter,
                "tab" => Key::Tab,
                "escape" => Key::Escape,
                "left" => Key::Left,
                "right" => Key::Right,
                "home" => Key::Home,
                "end" => Key::End,
                _ => return Err(format!("line {}: unknown key `{rest}`", number + 1)),
            }),
            "resize" if numbers.len() == 2 => {
                Event::Resize { width: numbers[0] as u32, height: numbers[1] as u32 }
            }
            "wait" => Event::Wait,
            "close" => Event::Close,
            _ => return Err(format!("line {} of the events: `{line}` is not an event", number + 1)),
        };
        events.push_back(event);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_read_from_lines() {
        let events =
            parse("# a test\nclick Charger\nclick 4 5\ntype a b\nkey enter\nwait\nresize 10 20\nclose\n")
                .unwrap();
        assert_eq!(
            Vec::from(events),
            [
                Event::ClickText("Charger".to_string()),
                Event::Click { x: 4, y: 5 },
                Event::Type("a b".to_string()),
                Event::Key(Key::Enter),
                Event::Wait,
                Event::Resize { width: 10, height: 20 },
                Event::Close,
            ]
        );
        assert!(parse("jump").is_err());
    }
}
