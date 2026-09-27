//! The image of a window, drawn by the runtime pixel by pixel: rectangles and texts.

use crate::font::Face;

/// Pixels of `0xRRGGBB`, row after row.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pixels: Vec<u32>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas { width, height, pixels: vec![0xFF_FF_FF; (width * height) as usize] }
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) != (self.width, self.height) {
            *self = Canvas::new(width, height);
        }
    }

    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    fn put(&mut self, x: i64, y: i64, color: u32) {
        if (0..i64::from(self.width)).contains(&x) && (0..i64::from(self.height)).contains(&y) {
            self.pixels[(y * i64::from(self.width) + x) as usize] = color;
        }
    }

    /// A filled rectangle; what falls outside the image is not drawn.
    pub fn fill(&mut self, x: i64, y: i64, width: i64, height: i64, color: u32) {
        let (left, right) = (x.max(0), (x + width).min(i64::from(self.width)));
        let (top, bottom) = (y.max(0), (y + height).min(i64::from(self.height)));
        for row in top..bottom {
            let start = (row * i64::from(self.width)) as usize;
            for column in left..right {
                self.pixels[start + column as usize] = color;
            }
        }
    }

    /// The border of a rectangle, one pixel wide.
    pub fn frame(&mut self, x: i64, y: i64, width: i64, height: i64, color: u32) {
        if width <= 0 || height <= 0 {
            return;
        }
        self.fill(x, y, width, 1, color);
        self.fill(x, y + height - 1, width, 1, color);
        self.fill(x, y, 1, height, color);
        self.fill(x + width - 1, y, 1, height, color);
    }

    /// A text whose top left corner is at `(x, y)`; a missing character is a box.
    pub fn text(&mut self, x: i64, y: i64, text: &str, face: &Face, color: u32) {
        let advance = i64::from(face.advance());
        let (width, height) = (face.font.width, face.height());
        for (position, c) in text.chars().enumerate() {
            let left = x + position as i64 * advance;
            let Some(rows) = face.glyph(c) else {
                self.frame(left + 1, y + 2, i64::from(width) - 2, i64::from(height) - 4, color);
                continue;
            };
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..width {
                    if bits >> (width - 1 - column) & 1 == 1 {
                        let (px, py) = (left + i64::from(column), y + row as i64);
                        self.put(px, py, color);
                        if face.embolden {
                            self.put(px + 1, py, color);
                        }
                    }
                }
            }
        }
    }

    /// The image in the PPM format (binary), for the tests.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for pixel in &self.pixels {
            out.extend([(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_stays_inside_the_image() {
        let mut canvas = Canvas::new(4, 3);
        canvas.fill(-2, 1, 4, 10, 0x12_34_56);
        assert_eq!(canvas.pixels()[4..8], [0x12_34_56, 0x12_34_56, 0xFF_FF_FF, 0xFF_FF_FF]);
        canvas.frame(0, 0, 4, 3, 0);
        assert_eq!(canvas.pixels()[5], 0x12_34_56);
        assert_eq!(canvas.pixels()[0], 0);
        let face = Face::new(13, false);
        let mut canvas = Canvas::new(20, 13);
        canvas.text(0, 0, "l", &face, 0);
        assert!(canvas.pixels().contains(&0));
        assert!(canvas.to_ppm().starts_with(b"P6\n20 13\n255\n"));
    }
}
