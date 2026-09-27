//! The windows of the library `ui` (§23, step 7, C98), for both modes: the image of a
//! window is drawn by the runtime, pixel by pixel, and shown through the X11 protocol;
//! with `LION_UI=headless`, nothing is shown and the events come from a file.
//!
//! The elements, their layout and the loop of events are written in Lion, in the module
//! `ui` of the standard library; this crate gives it windows, drawing and events.

mod canvas;
mod font;
mod font_data;
mod headless;
#[cfg(unix)]
mod x11;

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

pub use canvas::Canvas;
pub use font::Face;

/// What the user did.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Close,
    Click {
        x: i64,
        y: i64,
    },
    Key(Key),
    Resize {
        width: u32,
        height: u32,
    },
    /// The window must be drawn again.
    Redraw,
    /// Nothing happened in time.
    Timeout,
    /// Without a screen: a click on the element that shows this text.
    ClickText(String),
    /// Without a screen: a text typed.
    Type(String),
    /// Without a screen: the tasks that the interface waits for finish.
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    Left,
    Right,
    Home,
    End,
}

enum Backend {
    #[cfg(unix)]
    X11(x11::X11),
    Headless(headless::Headless),
}

struct Window {
    canvas: Canvas,
    backend: Backend,
}

fn windows() -> MutexGuard<'static, HashMap<i64, Window>> {
    static WINDOWS: OnceLock<Mutex<HashMap<i64, Window>>> = OnceLock::new();
    WINDOWS.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the interface runs without a screen: `LION_UI=headless`.
pub fn headless() -> bool {
    std::env::var("LION_UI").is_ok_and(|mode| mode == "headless")
}

/// Opens a window of this size; gives its number.
pub fn open(title: &str, width: u32, height: u32) -> Result<i64, String> {
    let (width, height) = (width.clamp(1, 8192), height.clamp(1, 8192));
    let backend = if headless() {
        Backend::Headless(headless::Headless::open()?)
    } else {
        #[cfg(unix)]
        {
            Backend::X11(x11::X11::open(title, width, height)?)
        }
        #[cfg(not(unix))]
        {
            let _ = title;
            return Err("cannot open a window: this version of Lion draws windows on Unix only".to_string());
        }
    };
    let mut windows = windows();
    let number = windows.keys().max().map_or(1, |last| last + 1);
    windows.insert(number, Window { canvas: Canvas::new(width, height), backend });
    Ok(number)
}

fn with<T>(window: i64, default: T, action: impl FnOnce(&mut Window) -> T) -> T {
    match windows().get_mut(&window) {
        Some(window) => action(window),
        None => default,
    }
}

/// The size of the window.
pub fn size(window: i64) -> (u32, u32) {
    with(window, (0, 0), |window| (window.canvas.width, window.canvas.height))
}

pub fn clear(window: i64, color: u32) {
    with(window, (), |window| window.canvas.clear(color));
}

pub fn fill(window: i64, x: i64, y: i64, width: i64, height: i64, color: u32) {
    with(window, (), |window| window.canvas.fill(x, y, width, height, color));
}

pub fn frame(window: i64, x: i64, y: i64, width: i64, height: i64, color: u32) {
    with(window, (), |window| window.canvas.frame(x, y, width, height, color));
}

pub fn text(window: i64, x: i64, y: i64, text: &str, size: i64, bold: bool, color: u32) {
    let face = Face::new(size, bold);
    with(window, (), |window| window.canvas.text(x, y, text, &face, color));
}

/// Shows what was drawn.
pub fn present(window: i64) -> Result<(), String> {
    with(window, Ok(()), |window| match &mut window.backend {
        #[cfg(unix)]
        Backend::X11(x11) => x11.present(&window.canvas),
        Backend::Headless(headless) => headless.present(&window.canvas),
    })
}

/// The next event; `timeout` in milliseconds, or none to wait as long as needed. A
/// change of size also resizes the image.
pub fn next_event(window: i64, timeout: Option<u64>) -> Result<Event, String> {
    let event = with(window, Ok(Event::Close), |window| match &mut window.backend {
        #[cfg(unix)]
        Backend::X11(x11) => x11.next_event(timeout.map(Duration::from_millis)),
        Backend::Headless(headless) => Ok(headless.next_event()),
    })?;
    if let Event::Resize { width, height } = event {
        with(window, (), |window| window.canvas.resize(width.clamp(1, 8192), height.clamp(1, 8192)));
    }
    Ok(event)
}

pub fn close(window: i64) {
    if let Some(mut window) = windows().remove(&window) {
        #[cfg(unix)]
        if let Backend::X11(x11) = &mut window.backend {
            x11.close();
        }
        let _ = &mut window;
    }
}

/// The width of a text in the font of this size.
pub fn text_width(text: &str, size: i64, bold: bool) -> i64 {
    i64::from(Face::new(size, bold).width_of(text))
}

/// The height of a line in the font of this size.
pub fn line_height(size: i64) -> i64 {
    i64::from(Face::new(size, false).height())
}
