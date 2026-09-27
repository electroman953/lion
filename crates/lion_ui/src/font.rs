//! The font of the interface: bitmaps of the public-domain "misc-fixed" fonts of X11,
//! embedded in `font_data.rs`, so that a window looks the same on every system and in
//! the tests (C98).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::font_data::{FONT_13, FONT_18, FONT_18_BOLD, FONT_20};

/// A font as `tools/extract_font.py` writes it.
pub struct FontData {
    pub width: u32,
    pub height: u32,
    pub ascent: u32,
    /// One line per character: its code point, `:`, then each row of its cell in
    /// hexadecimal.
    pub glyphs: &'static str,
}

/// A font ready to draw: the rows of each character, the leftmost pixel in the highest
/// bit of `width` bits.
pub struct Font {
    pub width: u32,
    pub height: u32,
    pub ascent: u32,
    glyphs: HashMap<char, Vec<u32>>,
}

impl Font {
    fn load(data: &FontData) -> Font {
        let digits = data.width.div_ceil(4) as usize;
        let mut glyphs = HashMap::new();
        for line in data.glyphs.lines() {
            let Some((code, rows)) = line.split_once(':') else { continue };
            let Some(c) = u32::from_str_radix(code, 16).ok().and_then(char::from_u32) else { continue };
            let rows = (0..rows.len() / digits)
                .map(|row| u32::from_str_radix(&rows[row * digits..(row + 1) * digits], 16).unwrap_or(0))
                .collect();
            glyphs.insert(c, rows);
        }
        Font { width: data.width, height: data.height, ascent: data.ascent, glyphs }
    }

    pub fn glyph(&self, c: char) -> Option<&[u32]> {
        self.glyphs.get(&c).map(Vec::as_slice)
    }
}

fn fonts() -> &'static [Font; 4] {
    static FONTS: OnceLock<[Font; 4]> = OnceLock::new();
    FONTS.get_or_init(|| {
        [Font::load(&FONT_13), Font::load(&FONT_18), Font::load(&FONT_18_BOLD), Font::load(&FONT_20)]
    })
}

/// The font for a size in pixels, bold or not: 13, 18 or 20 pixels high. A bold font
/// that does not exist is drawn twice, one pixel apart; a missing character is taken
/// from the regular font.
pub struct Face {
    pub font: &'static Font,
    pub fallback: &'static Font,
    pub embolden: bool,
}

impl Face {
    pub fn new(size: i64, bold: bool) -> Face {
        let [small, medium, medium_bold, large] = fonts();
        match size {
            ..16 => Face { font: small, fallback: small, embolden: bold },
            16..20 if bold => Face { font: medium_bold, fallback: medium, embolden: false },
            16..20 => Face { font: medium, fallback: medium, embolden: false },
            _ => Face { font: large, fallback: large, embolden: bold },
        }
    }

    /// The width of a character, the same for all.
    pub fn advance(&self) -> u32 {
        self.font.width + u32::from(self.embolden)
    }

    pub fn height(&self) -> u32 {
        self.font.height
    }

    pub fn glyph(&self, c: char) -> Option<&'static [u32]> {
        self.font.glyph(c).or_else(|| self.fallback.glyph(c))
    }

    /// The width of a text, in pixels.
    pub fn width_of(&self, text: &str) -> u32 {
        text.chars().count() as u32 * self.advance()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fonts_have_the_characters_of_lion_programs() {
        for size in [13, 18, 24] {
            let face = Face::new(size, true);
            for c in "Aé€π≤…".chars() {
                assert!(face.glyph(c).is_some(), "{c} at {size}");
            }
        }
        assert!(Face::new(18, false).glyph('✓').is_some());
        let face = Face::new(18, false);
        assert_eq!((face.advance(), face.height()), (9, 18));
        assert_eq!(face.width_of("Léa"), 27);
        assert_eq!(Face::new(24, true).advance(), 11);
    }
}
