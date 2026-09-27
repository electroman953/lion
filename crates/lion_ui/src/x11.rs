//! A client of the X11 protocol, written for Lion without any library (C98): it opens a
//! window, shows in it the images that the runtime draws, and reads the events. Under
//! Wayland, the window goes through XWayland.
//!
//! Only what `ui` needs is here: the connection and its authorization (MIT magic
//! cookie), a window with its title and the button that closes it, images in 24-bit
//! true color, and the keyboard, the mouse and the changes of size.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use crate::canvas::Canvas;
use crate::{Event, Key};

/// The atoms that every X server predefines.
const ATOM: u32 = 4;
const STRING: u32 = 31;
const WM_NAME: u32 = 39;
const WM_CLASS: u32 = 67;

/// The events that the window asks for: keys, mouse buttons, exposure, size.
const EVENT_MASK: u32 = 0x1 | 0x4 | 0x8000 | 0x2_0000;

enum Stream {
    #[cfg(unix)]
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Stream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(stream) => stream.set_read_timeout(timeout),
            Stream::Tcp(stream) => stream.set_read_timeout(timeout),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(stream) => stream.read(buffer),
            Stream::Tcp(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(stream) => stream.write(buffer),
            Stream::Tcp(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(stream) => stream.flush(),
            Stream::Tcp(stream) => stream.flush(),
        }
    }
}

pub struct X11 {
    stream: Stream,
    /// The number of the last request sent, as the server counts them.
    sequence: u16,
    id_base: u32,
    id_mask: u32,
    next_id: u32,
    root: u32,
    depth: u8,
    window: u32,
    gc: u32,
    width: u32,
    height: u32,
    /// The largest request, in bytes.
    max_request: usize,
    wm_protocols: u32,
    wm_delete: u32,
    min_keycode: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
    /// The events read while waiting for a reply.
    queue: VecDeque<Vec<u8>>,
}

fn problem(what: &str, error: io::Error) -> String {
    format!("{what}: {error}")
}

impl X11 {
    /// Opens a window on the display named by `DISPLAY`.
    pub fn open(title: &str, width: u32, height: u32) -> Result<X11, String> {
        let display = std::env::var("DISPLAY").map_err(|_| {
            "cannot open a window: no display (DISPLAY is not set); LION_UI=headless runs without one"
                .to_string()
        })?;
        let (host, number) = parse_display(&display)?;
        let stream = connect(&host, number)?;
        let (auth_name, auth_data) = xauthority(number).unwrap_or_default();
        let mut x = X11 {
            stream,
            sequence: 0,
            id_base: 0,
            id_mask: 0,
            next_id: 0,
            root: 0,
            depth: 24,
            window: 0,
            gc: 0,
            width,
            height,
            max_request: 262_140,
            wm_protocols: 0,
            wm_delete: 0,
            min_keycode: 8,
            keysyms_per_keycode: 0,
            keysyms: Vec::new(),
            queue: VecDeque::new(),
        };
        x.setup(&auth_name, &auth_data)?;
        x.create_window(title)?;
        Ok(x)
    }

    /// The connection setup (the X Window System Protocol, chapter 8).
    fn setup(&mut self, auth_name: &[u8], auth_data: &[u8]) -> Result<(), String> {
        let mut request = vec![b'l', 0];
        request.extend(11u16.to_le_bytes());
        request.extend(0u16.to_le_bytes());
        request.extend((auth_name.len() as u16).to_le_bytes());
        request.extend((auth_data.len() as u16).to_le_bytes());
        request.extend([0, 0]);
        request.extend(auth_name);
        pad(&mut request);
        request.extend(auth_data);
        pad(&mut request);
        self.stream.write_all(&request).map_err(|error| problem("cannot talk to the display", error))?;
        let mut head = [0u8; 8];
        self.stream.read_exact(&mut head).map_err(|error| problem("the display does not answer", error))?;
        let mut body = vec![0u8; usize::from(u16::from_le_bytes([head[6], head[7]])) * 4];
        self.stream.read_exact(&mut body).map_err(|error| problem("the display does not answer", error))?;
        match head[0] {
            1 => {}
            0 => {
                let reason =
                    String::from_utf8_lossy(&body[..usize::from(head[1]).min(body.len())]).into_owned();
                return Err(format!("the display refused the connection: {}", reason.trim()));
            }
            _ => return Err("the display asks for an authorization that Lion does not know".to_string()),
        }
        let u16_at = |at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
        self.id_base = u32_at(4);
        self.id_mask = u32_at(8);
        let vendor = usize::from(u16_at(16));
        self.max_request = usize::from(u16_at(18)) * 4;
        let formats = usize::from(body[21]);
        let msb_images = body[22] == 1;
        self.min_keycode = body[26];
        let max_keycode = body[27];
        let formats_at = 32 + vendor.div_ceil(4) * 4;
        let screen = formats_at + 8 * formats;
        self.root = u32_at(screen);
        let root_visual = u32_at(screen + 32);
        self.depth = body[screen + 38];
        let bits_per_pixel = (0..formats)
            .map(|index| formats_at + 8 * index)
            .find(|&at| body[at] == self.depth)
            .map(|at| body[at + 1]);
        // The visual of the root window must hold 8 bits of red, green and blue.
        let mut at = screen + 40;
        let mut true_color = false;
        for _ in 0..body[screen + 39] {
            let visuals = usize::from(u16_at(at + 2));
            at += 8;
            for _ in 0..visuals {
                if u32_at(at) == root_visual {
                    true_color = matches!(body[at + 4], 4 | 5)
                        && (u32_at(at + 8), u32_at(at + 12), u32_at(at + 16)) == (0xFF_0000, 0xFF00, 0xFF);
                }
                at += 24;
            }
        }
        if !true_color || bits_per_pixel != Some(32) || msb_images {
            return Err(
                "the display uses colors that Lion cannot draw yet (24-bit true color is needed)".to_string()
            );
        }
        self.keyboard(max_keycode)
    }

    fn new_id(&mut self) -> u32 {
        let shift = self.id_mask.trailing_zeros();
        self.next_id += 1;
        self.id_base | ((self.next_id << shift) & self.id_mask)
    }

    fn send(&mut self, opcode: u8, detail: u8, body: &[u8]) -> Result<u16, String> {
        let mut request = Vec::with_capacity(4 + body.len() + 3);
        request.extend([opcode, detail, 0, 0]);
        request.extend(body);
        pad(&mut request);
        let length = (request.len() / 4) as u16;
        request[2..4].copy_from_slice(&length.to_le_bytes());
        self.stream
            .write_all(&request)
            .map_err(|error| problem("the display closed the connection", error))?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(self.sequence)
    }

    /// The next packet from the server (a reply, an event or an error), or `None` when
    /// nothing comes before the deadline.
    fn read_packet(&mut self, deadline: Option<Instant>) -> Result<Option<Vec<u8>>, String> {
        let timeout = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(None);
                }
                Some(left)
            }
            None => None,
        };
        self.stream.set_read_timeout(timeout).map_err(|error| problem("the display", error))?;
        let mut packet = vec![0u8; 32];
        match self.stream.read(&mut packet[..1]) {
            Ok(0) => return Err("the display closed the connection".to_string()),
            Ok(_) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                return Ok(None);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(None),
            Err(error) => return Err(problem("the display", error)),
        }
        // The rest of a packet comes at once.
        self.stream.set_read_timeout(None).map_err(|error| problem("the display", error))?;
        self.stream.read_exact(&mut packet[1..]).map_err(|error| problem("the display", error))?;
        if packet[0] == 1 {
            let extra = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]) as usize * 4;
            packet.resize(32 + extra, 0);
            self.stream.read_exact(&mut packet[32..]).map_err(|error| problem("the display", error))?;
        }
        Ok(Some(packet))
    }

    /// The reply to the request `sequence`; the events met meanwhile are kept.
    fn reply(&mut self, sequence: u16) -> Result<Vec<u8>, String> {
        loop {
            let packet = self.read_packet(None)?.expect("no deadline");
            match packet[0] {
                0 => return Err(refusal(&packet)),
                1 if u16::from_le_bytes([packet[2], packet[3]]) == sequence => return Ok(packet),
                1 => {}
                _ => self.queue.push_back(packet),
            }
        }
    }

    fn intern(&mut self, name: &str) -> Result<u32, String> {
        let mut body = Vec::new();
        body.extend((name.len() as u16).to_le_bytes());
        body.extend([0, 0]);
        body.extend(name.as_bytes());
        let sequence = self.send(16, 0, &body)?;
        let reply = self.reply(sequence)?;
        Ok(u32::from_le_bytes([reply[8], reply[9], reply[10], reply[11]]))
    }

    fn change_property(&mut self, property: u32, kind: u32, format: u8, data: &[u8]) -> Result<(), String> {
        let mut body = Vec::new();
        body.extend(self.window.to_le_bytes());
        body.extend(property.to_le_bytes());
        body.extend(kind.to_le_bytes());
        body.extend([format, 0, 0, 0]);
        body.extend(((data.len() / usize::from(format / 8)) as u32).to_le_bytes());
        body.extend(data);
        self.send(18, 0, &body).map(|_| ())
    }

    fn create_window(&mut self, title: &str) -> Result<(), String> {
        self.window = self.new_id();
        let mut body = Vec::new();
        body.extend(self.window.to_le_bytes());
        body.extend(self.root.to_le_bytes());
        body.extend([0u8; 4]);
        body.extend((self.width as u16).to_le_bytes());
        body.extend((self.height as u16).to_le_bytes());
        body.extend(0u16.to_le_bytes());
        body.extend(1u16.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend(0x800u32.to_le_bytes());
        body.extend(EVENT_MASK.to_le_bytes());
        self.send(1, 0, &body)?;
        self.wm_protocols = self.intern("WM_PROTOCOLS")?;
        self.wm_delete = self.intern("WM_DELETE_WINDOW")?;
        let net_wm_name = self.intern("_NET_WM_NAME")?;
        let utf8 = self.intern("UTF8_STRING")?;
        let latin1: Vec<u8> = title.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        self.change_property(WM_NAME, STRING, 8, &latin1)?;
        self.change_property(net_wm_name, utf8, 8, title.as_bytes())?;
        self.change_property(WM_CLASS, STRING, 8, b"lion\0Lion\0")?;
        let delete = self.wm_delete.to_le_bytes();
        self.change_property(self.wm_protocols, ATOM, 32, &delete)?;
        self.gc = self.new_id();
        let mut body = Vec::new();
        body.extend(self.gc.to_le_bytes());
        body.extend(self.window.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        self.send(55, 0, &body)?;
        self.send(8, 0, &self.window.to_le_bytes())?;
        Ok(())
    }

    /// The keys of the keyboard and the symbols they give.
    fn keyboard(&mut self, max_keycode: u8) -> Result<(), String> {
        let count = max_keycode - self.min_keycode + 1;
        let sequence = self.send(101, 0, &[self.min_keycode, count, 0, 0])?;
        let reply = self.reply(sequence)?;
        self.keysyms_per_keycode = usize::from(reply[1]);
        self.keysyms =
            reply[32..].as_chunks::<4>().0.iter().map(|bytes| u32::from_le_bytes(*bytes)).collect();
        Ok(())
    }

    /// Shows the image in the window, in strips that fit a request.
    pub fn present(&mut self, canvas: &Canvas) -> Result<(), String> {
        let row_bytes = canvas.width as usize * 4;
        if row_bytes == 0 {
            return Ok(());
        }
        let rows_per_strip = ((self.max_request.min(262_140) - 24) / row_bytes).max(1);
        let pixels = canvas.pixels();
        let mut top = 0;
        while top < canvas.height as usize {
            let rows = rows_per_strip.min(canvas.height as usize - top);
            let mut body = Vec::with_capacity(20 + rows * row_bytes);
            body.extend(self.window.to_le_bytes());
            body.extend(self.gc.to_le_bytes());
            body.extend((canvas.width as u16).to_le_bytes());
            body.extend((rows as u16).to_le_bytes());
            body.extend(0i16.to_le_bytes());
            body.extend((top as i16).to_le_bytes());
            body.extend([0, self.depth, 0, 0]);
            let start = top * canvas.width as usize;
            for pixel in &pixels[start..start + rows * canvas.width as usize] {
                body.extend(pixel.to_le_bytes());
            }
            self.send(72, 2, &body)?;
            top += rows;
        }
        Ok(())
    }

    /// The next event, or `Event::Timeout` when none comes in time.
    pub fn next_event(&mut self, timeout: Option<Duration>) -> Result<Event, String> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            let packet = match self.queue.pop_front() {
                Some(packet) => packet,
                None => match self.read_packet(deadline)? {
                    Some(packet) => packet,
                    None => return Ok(Event::Timeout),
                },
            };
            if let Some(event) = self.translate(&packet)? {
                return Ok(event);
            }
        }
    }

    fn translate(&mut self, packet: &[u8]) -> Result<Option<Event>, String> {
        // LION_UI_DEBUG shows what the display sends, to understand a window that stays empty.
        if std::env::var_os("LION_UI_DEBUG").is_some() {
            eprintln!(
                "[x11] packet {} ({} bytes): {:?}",
                packet[0],
                packet.len(),
                &packet[..packet.len().min(32)]
            );
        }
        let u16_at = |at: usize| u16::from_le_bytes([packet[at], packet[at + 1]]);
        let i16_at = |at: usize| i16::from_le_bytes([packet[at], packet[at + 1]]);
        Ok(match packet[0] & 0x7F {
            0 => return Err(refusal(packet)),
            2 => self.key(packet[1], u16_at(28)).map(Event::Key),
            4 if packet[1] == 1 => Some(Event::Click { x: i64::from(i16_at(24)), y: i64::from(i16_at(26)) }),
            // Shown, or uncovered: the image is sent again.
            12 if u16_at(16) == 0 => Some(Event::Redraw),
            19 => Some(Event::Redraw),
            22 => {
                let (width, height) = (u32::from(u16_at(20)), u32::from(u16_at(22)));
                if (width, height) == (self.width, self.height) {
                    None
                } else {
                    self.width = width;
                    self.height = height;
                    Some(Event::Resize { width, height })
                }
            }
            33 if u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]) == self.wm_delete => {
                Some(Event::Close)
            }
            34 => {
                let max =
                    self.min_keycode as usize + self.keysyms.len() / self.keysyms_per_keycode.max(1) - 1;
                self.keyboard(max as u8)?;
                None
            }
            _ => None,
        })
    }

    /// The key of a keycode, with the state of the modifiers: Shift, Caps Lock, and the
    /// third level (AltGr), which Mod5 usually gives.
    fn key(&self, keycode: u8, state: u16) -> Option<Key> {
        let per = self.keysyms_per_keycode;
        let start = usize::from(keycode.checked_sub(self.min_keycode)?) * per;
        let symbols = self.keysyms.get(start..start + per)?;
        let shift = state & 1 != 0;
        let caps = state & 2 != 0;
        let group = if state & 0x80 != 0 && per > 4 { 4 } else { 0 };
        let pick = |column: usize| symbols.get(column).copied().filter(|&symbol| symbol != 0);
        let symbol = if shift { pick(group + 1).or_else(|| pick(group)) } else { pick(group) }?;
        let key = key_of_symbol(symbol)?;
        Some(match key {
            Key::Char(c) if caps && !shift && c.is_lowercase() => {
                Key::Char(c.to_uppercase().next().unwrap_or(c))
            }
            key => key,
        })
    }

    pub fn close(&mut self) {
        let window = self.window.to_le_bytes();
        let _ = self.send(4, 0, &window);
        let _ = self.stream.flush();
    }
}

/// The key of a keysym (the X Window System Protocol, appendix A).
fn key_of_symbol(symbol: u32) -> Option<Key> {
    Some(match symbol {
        0xFF08 => Key::Backspace,
        0xFF09 => Key::Tab,
        0xFF0D | 0xFF8D => Key::Enter,
        0xFF1B => Key::Escape,
        0xFFFF => Key::Delete,
        0xFF50 => Key::Home,
        0xFF51 => Key::Left,
        0xFF53 => Key::Right,
        0xFF57 => Key::End,
        0x20..=0x7E | 0xA0..=0xFF => Key::Char(char::from(symbol as u8)),
        0x0100_0000..=0x0110_FFFF => Key::Char(char::from_u32(symbol - 0x0100_0000)?),
        0x20AC => Key::Char('€'),
        _ => return None,
    })
}

fn refusal(packet: &[u8]) -> String {
    format!("the display refused a request of Lion (error {}, request {})", packet[1], packet[10])
}

fn pad(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

/// `host:number.screen`: the host (empty for this machine) and the number of the display.
fn parse_display(display: &str) -> Result<(String, u16), String> {
    let wrong = || format!("cannot open a window: DISPLAY is `{display}`, which does not name a display");
    let (host, rest) = display.rsplit_once(':').ok_or_else(wrong)?;
    let number = rest.split('.').next().unwrap_or("").parse().map_err(|_| wrong())?;
    Ok((host.to_string(), number))
}

fn connect(host: &str, number: u16) -> Result<Stream, String> {
    let unreachable =
        |error: io::Error| format!("cannot open a window: the display :{number} does not answer ({error})");
    if host.is_empty() || host == "unix" {
        #[cfg(unix)]
        return UnixStream::connect(format!("/tmp/.X11-unix/X{number}"))
            .map(Stream::Unix)
            .map_err(unreachable);
    }
    TcpStream::connect((host, 6000 + number)).map(Stream::Tcp).map_err(unreachable)
}

/// The MIT magic cookie of the display, from the file named by `XAUTHORITY`, or
/// `~/.Xauthority`: each entry holds a family, an address, a display number, the name
/// of the method and its data, each with its length on two bytes (big endian).
fn xauthority(number: u16) -> Option<(Vec<u8>, Vec<u8>)> {
    let path = match std::env::var_os("XAUTHORITY") {
        Some(path) => std::path::PathBuf::from(path),
        None => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".Xauthority"),
    };
    let data = std::fs::read(path).ok()?;
    let mut at = 0;
    let field = |at: &mut usize| -> Option<Vec<u8>> {
        let length = usize::from(u16::from_be_bytes([*data.get(*at)?, *data.get(*at + 1)?]));
        let value = data.get(*at + 2..*at + 2 + length)?.to_vec();
        *at += 2 + length;
        Some(value)
    };
    let wanted = number.to_string().into_bytes();
    while at + 2 <= data.len() {
        let family = u16::from_be_bytes([data[at], data[at + 1]]);
        at += 2;
        let _address = field(&mut at)?;
        let display = field(&mut at)?;
        let name = field(&mut at)?;
        let cookie = field(&mut at)?;
        let local = matches!(family, 256 | 0xFFFF | 0);
        if local && (display == wanted || display.is_empty()) && name == b"MIT-MAGIC-COOKIE-1" {
            return Some((name, cookie));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_are_parsed() {
        assert_eq!(parse_display(":0"), Ok((String::new(), 0)));
        assert_eq!(parse_display("unix:1.0"), Ok(("unix".to_string(), 1)));
        assert_eq!(parse_display("localhost:10.0"), Ok(("localhost".to_string(), 10)));
        assert!(parse_display("nothing").is_err());
    }

    #[test]
    fn keysyms_give_keys() {
        assert_eq!(key_of_symbol(0x61), Some(Key::Char('a')));
        assert_eq!(key_of_symbol(0xE9), Some(Key::Char('é')));
        assert_eq!(key_of_symbol(0x0100_03C0), Some(Key::Char('π')));
        assert_eq!(key_of_symbol(0xFF08), Some(Key::Backspace));
        assert_eq!(key_of_symbol(0xFFE1), None);
    }
}
