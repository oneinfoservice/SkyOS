//! Compositor — layer-based rendering pipeline.
//!
//! Owns six screen-sized pixel buffers (one per [`Layer`]), each allocated
//! once and reused every frame. Drawing modules write into a layer's buffer
//! via [`Canvas`]; then [`Compositor::compose`] alpha-blends the layers onto
//! the real window framebuffer in draw order.
//!
//! The embedded 8×8 bitmap font for [`Canvas::draw_char`] / [`Canvas::draw_string`]
//! matches the CP437 glyphs used by libsarga's own `draw_char` so that text
//! rendering stays pixel-identical to the previous direct-draw pipeline.

use crate::core::geometry::Rect;
use crate::render::layer::{Layer, LAYER_COUNT};
use alloc::vec::Vec;

// ── Pixel helpers ──────────────────────────────────────────────────────────

#[inline]
fn alpha_blend(bg: u32, fg: u32, alpha: u8) -> u32 {
    if alpha == 0 {
        return bg;
    }
    if alpha == 255 {
        return fg;
    }
    let a = alpha as u32;
    let inv_a = 255 - a;
    let r = (((fg >> 16) & 0xFF) * a + ((bg >> 16) & 0xFF) * inv_a) / 255;
    let g = (((fg >> 8) & 0xFF) * a + ((bg >> 8) & 0xFF) * inv_a) / 255;
    let b = ((fg & 0xFF) * a + (bg & 0xFF) * inv_a) / 255;
    (r << 16) | (g << 8) | b
}

// ── Canvas ─────────────────────────────────────────────────────────────────

/// Mutable view into a screen-sized pixel buffer.
///
/// Provides the same drawing API that `libsarga::gui::Window` exposes, but
/// operates on an arbitrary `&mut [u32]` slice instead of the kernel-mapped
/// window buffer.  All methods clamp to `(w, h)` so callers never write out
/// of bounds.
pub(crate) struct Canvas<'a> {
    pub data: &'a mut [u32],
    pub w: u32,
    pub h: u32,
}

impl<'a> Canvas<'a> {
    pub fn fill_rect(&mut self, x: u32, y: u32, rw: u32, rh: u32, color: u32) {
        let sw = self.w as usize;
        let sh = self.h as usize;
        let x0 = x.min(sw as u32) as usize;
        let y0 = y.min(sh as u32) as usize;
        let x1 = (x + rw).min(sw as u32) as usize;
        let y1 = (y + rh).min(sh as u32) as usize;
        for py in y0..y1 {
            let row = py * sw;
            for px in x0..x1 {
                self.data[row + px] = color;
            }
        }
    }

    pub fn fill_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.w && y < self.h {
            self.data[(y * self.w + x) as usize] = color;
        }
    }

    pub fn draw_rect_alpha(&mut self, x: u32, y: u32, rw: u32, rh: u32, color: u32) {
        let a = ((color >> 24) & 0xFF) as u8;
        if a == 0 {
            return;
        }
        if a == 255 {
            self.fill_rect(x, y, rw, rh, color);
            return;
        }
        let sw = self.w as usize;
        let sh = self.h as usize;
        let x0 = x.min(sw as u32) as usize;
        let y0 = y.min(sh as u32) as usize;
        let x1 = (x + rw).min(sw as u32) as usize;
        let y1 = (y + rh).min(sh as u32) as usize;
        for py in y0..y1 {
            let row = py * sw;
            for px in x0..x1 {
                self.data[row + px] = alpha_blend(self.data[row + px], color, a);
            }
        }
    }

    pub fn draw_rect_outline(&mut self, x: u32, y: u32, rw: u32, rh: u32, color: u32) {
        self.draw_line_h(x, y, rw, color);
        self.draw_line_h(x, y + rh - 1, rw, color);
        self.draw_line_v(x, y, rh, color);
        self.draw_line_v(x + rw - 1, y, rh, color);
    }

    pub fn draw_rounded_rect(&mut self, x: u32, y: u32, rw: u32, rh: u32, radius: u32, color: u32) {
        let r = radius.min(rw / 2).min(rh / 2) as i32;
        if r <= 0 {
            self.fill_rect(x, y, rw, rh, color);
            return;
        }
        self.fill_rect(x + r as u32, y, rw - 2 * r as u32, rh, color);
        self.fill_rect(x, y + r as u32, r as u32, rh - 2 * r as u32, color);
        self.fill_rect(
            x + rw - r as u32,
            y + r as u32,
            r as u32,
            rh - 2 * r as u32,
            color,
        );
        for dy in 0..r {
            for dx in 0..r {
                let r2 = r * r;
                if (r - dx - 1) * (r - dx - 1) + (r - dy - 1) * (r - dy - 1) <= r2 {
                    self.fill_pixel(x + dx as u32, y + dy as u32, color);
                }
                if (dx) * (dx) + (r - dy - 1) * (r - dy - 1) <= r2 {
                    self.fill_pixel(x + rw - r as u32 + dx as u32, y + dy as u32, color);
                }
                if (r - dx - 1) * (r - dx - 1) + (dy) * (dy) <= r2 {
                    self.fill_pixel(x + dx as u32, y + rh - r as u32 + dy as u32, color);
                }
                if (dx) * (dx) + (dy) * (dy) <= r2 {
                    self.fill_pixel(
                        x + rw - r as u32 + dx as u32,
                        y + rh - r as u32 + dy as u32,
                        color,
                    );
                }
            }
        }
    }

    pub fn draw_rounded_rect_outline(
        &mut self,
        x: u32,
        y: u32,
        rw: u32,
        rh: u32,
        radius: u32,
        color: u32,
    ) {
        let r = radius.min(rw / 2).min(rh / 2) as i32;
        if r <= 0 {
            self.draw_rect_outline(x, y, rw, rh, color);
            return;
        }
        self.draw_line_h(x + r as u32, y, rw - 2 * r as u32, color);
        self.draw_line_h(x + r as u32, y + rh - 1, rw - 2 * r as u32, color);
        self.draw_line_v(x, y + r as u32, rh - 2 * r as u32, color);
        self.draw_line_v(x + rw - 1, y + r as u32, rh - 2 * r as u32, color);

        let mut cx = 0;
        let mut cy = r;
        let mut d = 3 - 2 * r;
        while cx <= cy {
            self.fill_pixel(x + (r - cx) as u32, y + (r - cy) as u32, color);
            self.fill_pixel(x + (r - cy) as u32, y + (r - cx) as u32, color);
            self.fill_pixel(x + rw - 1 - (r - cx) as u32, y + (r - cy) as u32, color);
            self.fill_pixel(x + rw - 1 - (r - cy) as u32, y + (r - cx) as u32, color);
            self.fill_pixel(x + (r - cx) as u32, y + rh - 1 - (r - cy) as u32, color);
            self.fill_pixel(x + (r - cy) as u32, y + rh - 1 - (r - cx) as u32, color);
            self.fill_pixel(
                x + rw - 1 - (r - cx) as u32,
                y + rh - 1 - (r - cy) as u32,
                color,
            );
            self.fill_pixel(
                x + rw - 1 - (r - cy) as u32,
                y + rh - 1 - (r - cx) as u32,
                color,
            );
            if d < 0 {
                d += 4 * cx + 6;
            } else {
                d += 4 * (cx - cy) + 10;
                cy -= 1;
            }
            cx += 1;
        }
    }

    pub fn draw_rect(&mut self, x: u32, y: u32, rw: u32, rh: u32, color: u32) {
        self.fill_rect(x, y, rw, rh, color);
    }

    #[allow(clippy::too_many_arguments)] // public canvas API, matches sibling draw fns
    pub fn draw_gradient_rect(
        &mut self,
        x: u32,
        y: u32,
        rw: u32,
        rh: u32,
        color1: u32,
        color2: u32,
        vertical: bool,
    ) {
        let sw = self.w as usize;
        let sh = self.h as usize;
        let x0 = x.min(sw as u32) as usize;
        let y0 = y.min(sh as u32) as usize;
        let x1 = (x + rw).min(sw as u32) as usize;
        let y1 = (y + rh).min(sh as u32) as usize;

        let r1 = ((color1 >> 16) & 0xFF) as i32;
        let g1 = ((color1 >> 8) & 0xFF) as i32;
        let b1 = (color1 & 0xFF) as i32;
        let a1 = ((color1 >> 24) & 0xFF) as i32;
        let r2 = ((color2 >> 16) & 0xFF) as i32;
        let g2 = ((color2 >> 8) & 0xFF) as i32;
        let b2 = (color2 & 0xFF) as i32;
        let a2 = ((color2 >> 24) & 0xFF) as i32;

        if vertical {
            for py in y0..y1 {
                let t = (py - y0) as f32 / (y1 - y0).max(1) as f32;
                let r = (r1 as f32 + t * (r2 - r1) as f32) as u32;
                let g = (g1 as f32 + t * (g2 - g1) as f32) as u32;
                let b = (b1 as f32 + t * (b2 - b1) as f32) as u32;
                let a = (a1 as f32 + t * (a2 - a1) as f32) as u32;
                let color = (a << 24) | (r << 16) | (g << 8) | b;
                let row = py * sw;
                for px in x0..x1 {
                    self.data[row + px] = color;
                }
            }
        } else {
            for px in x0..x1 {
                let t = (px - x0) as f32 / (x1 - x0).max(1) as f32;
                let r = (r1 as f32 + t * (r2 - r1) as f32) as u32;
                let g = (g1 as f32 + t * (g2 - g1) as f32) as u32;
                let b = (b1 as f32 + t * (b2 - b1) as f32) as u32;
                let a = (a1 as f32 + t * (a2 - a1) as f32) as u32;
                let color = (a << 24) | (r << 16) | (g << 8) | b;
                for py in y0..y1 {
                    self.data[py * sw + px] = color;
                }
            }
        }
    }

    pub fn draw_line_h(&mut self, x: u32, y: u32, len: u32, color: u32) {
        let sw = self.w as usize;
        let sh = self.h as usize;
        let x0 = x.min(sw as u32) as usize;
        let x1 = (x + len).min(sw as u32) as usize;
        if y as usize >= sh {
            return;
        }
        let row = y as usize * sw;
        for px in x0..x1 {
            self.data[row + px] = color;
        }
    }

    pub fn draw_line_v(&mut self, x: u32, y: u32, len: u32, color: u32) {
        let sw = self.w as usize;
        let sh = self.h as usize;
        if x as usize >= sw {
            return;
        }
        let y0 = y.min(sh as u32) as usize;
        let y1 = (y + len).min(sh as u32) as usize;
        for py in y0..y1 {
            self.data[py * sw + x as usize] = color;
        }
    }

    // ── font8x8 glyph data (CP437, ASCII 32–126) ──────────────────────

    /// Return the 8-byte glyph bitmap for a printable ASCII character.
    /// Characters outside 32–126 yield an all-zero glyph (blank).
    fn glyph(c: u8) -> &'static [u8; 8] {
        // The glyph table is 95 entries × 8 bytes, indexed by (c - 32).
        const GLYPHS: &[[u8; 8]; 95] = &[
            // 32 ' '
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            // 33 '!'
            [0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x00],
            // 34 '"'
            [0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00],
            // 35 '#'
            [0x66, 0x66, 0xFF, 0x66, 0xFF, 0x66, 0x66, 0x00],
            // 36 '$'
            [0x18, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x18, 0x00],
            // 37 '%'
            [0x62, 0x66, 0x0C, 0x18, 0x30, 0x66, 0x46, 0x00],
            // 38 '&'
            [0x3C, 0x66, 0x3C, 0x38, 0x67, 0x66, 0x3F, 0x00],
            // 39 '\''
            [0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00],
            // 40 '('
            [0x0C, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00],
            // 41 ')'
            [0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00],
            // 42 '*'
            [0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00],
            // 43 '+'
            [0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00],
            // 44 ','
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30],
            // 45 '-'
            [0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00],
            // 46 '.'
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00],
            // 47 '/'
            [0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0x80, 0x00],
            // 48 '0'
            [0x3C, 0x66, 0x6E, 0x7E, 0x76, 0x66, 0x3C, 0x00],
            // 49 '1'
            [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00],
            // 50 '2'
            [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x30, 0x7E, 0x00],
            // 51 '3'
            [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0x00],
            // 52 '4'
            [0x0C, 0x1C, 0x3C, 0x6C, 0x7E, 0x0C, 0x0C, 0x00],
            // 53 '5'
            [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0x00],
            // 54 '6'
            [0x3C, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x3C, 0x00],
            // 55 '7'
            [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x00],
            // 56 '8'
            [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0x00],
            // 57 '9'
            [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x0C, 0x38, 0x00],
            // 58 ':'
            [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00],
            // 59 ';'
            [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x30],
            // 60 '<'
            [0x0C, 0x18, 0x30, 0x60, 0x30, 0x18, 0x0C, 0x00],
            // 61 '='
            [0x00, 0x00, 0x7E, 0x00, 0x00, 0x7E, 0x00, 0x00],
            // 62 '>'
            [0x30, 0x18, 0x0C, 0x06, 0x0C, 0x18, 0x30, 0x00],
            // 63 '?'
            [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x00, 0x18, 0x00],
            // 64 '@'
            [0x3C, 0x66, 0x6E, 0x6E, 0x60, 0x62, 0x3C, 0x00],
            // 65 'A'
            [0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x00],
            // 66 'B'
            [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0x00],
            // 67 'C'
            [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00],
            // 68 'D'
            [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0x00],
            // 69 'E'
            [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x7E, 0x00],
            // 70 'F'
            [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0x00],
            // 71 'G'
            [0x3C, 0x66, 0x60, 0x6E, 0x66, 0x66, 0x3E, 0x00],
            // 72 'H'
            [0x66, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x00],
            // 73 'I'
            [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00],
            // 74 'J'
            [0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x6C, 0x38, 0x00],
            // 75 'K'
            [0x66, 0x6C, 0x78, 0x70, 0x78, 0x6C, 0x66, 0x00],
            // 76 'L'
            [0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7E, 0x00],
            // 77 'M'
            [0x63, 0x77, 0x7F, 0x6B, 0x63, 0x63, 0x63, 0x00],
            // 78 'N'
            [0x66, 0x76, 0x7E, 0x7E, 0x6E, 0x66, 0x66, 0x00],
            // 79 'O'
            [0x3C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
            // 80 'P'
            [0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x00],
            // 81 'Q'
            [0x3C, 0x66, 0x66, 0x66, 0x66, 0x6C, 0x36, 0x00],
            // 82 'R'
            [0x7C, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0x66, 0x00],
            // 83 'S'
            [0x3C, 0x66, 0x60, 0x3C, 0x06, 0x66, 0x3C, 0x00],
            // 84 'T'
            [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
            // 85 'U'
            [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
            // 86 'V'
            [0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00],
            // 87 'W'
            [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00],
            // 88 'X'
            [0x66, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x66, 0x00],
            // 89 'Y'
            [0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x18, 0x00],
            // 90 'Z'
            [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x7E, 0x00],
            // 91 '['
            [0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00],
            // 92 '\'
            [0xC0, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x02, 0x00],
            // 93 ']'
            [0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00],
            // 94 '^'
            [0x18, 0x3C, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00],
            // 95 '_'
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF],
            // 96 '`'
            [0x30, 0x18, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00],
            // 97 'a'
            [0x00, 0x00, 0x3C, 0x06, 0x3E, 0x66, 0x3E, 0x00],
            // 98 'b'
            [0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x7C, 0x00],
            // 99 'c'
            [0x00, 0x00, 0x3C, 0x60, 0x60, 0x60, 0x3C, 0x00],
            // 100 'd'
            [0x06, 0x06, 0x3E, 0x66, 0x66, 0x66, 0x3E, 0x00],
            // 101 'e'
            [0x00, 0x00, 0x3C, 0x66, 0x7E, 0x60, 0x3C, 0x00],
            // 102 'f'
            [0x1C, 0x36, 0x30, 0x78, 0x30, 0x30, 0x30, 0x00],
            // 103 'g'
            [0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x06, 0x3C],
            // 104 'h'
            [0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x00],
            // 105 'i'
            [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3C, 0x00],
            // 106 'j'
            [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x18, 0x70],
            // 107 'k'
            [0x60, 0x60, 0x66, 0x6C, 0x78, 0x6C, 0x66, 0x00],
            // 108 'l'
            [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
            // 109 'm'
            [0x00, 0x00, 0x66, 0x7F, 0x7F, 0x6B, 0x63, 0x00],
            // 110 'n'
            [0x00, 0x00, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x00],
            // 111 'o'
            [0x00, 0x00, 0x3C, 0x66, 0x66, 0x66, 0x3C, 0x00],
            // 112 'p'
            [0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60],
            // 113 'q'
            [0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x06, 0x06],
            // 114 'r'
            [0x00, 0x00, 0x7C, 0x66, 0x60, 0x60, 0x60, 0x00],
            // 115 's'
            [0x00, 0x00, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x00],
            // 116 't'
            [0x30, 0x30, 0x7C, 0x30, 0x30, 0x34, 0x18, 0x00],
            // 117 'u'
            [0x00, 0x00, 0x66, 0x66, 0x66, 0x66, 0x3E, 0x00],
            // 118 'v'
            [0x00, 0x00, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00],
            // 119 'w'
            [0x00, 0x00, 0x63, 0x6B, 0x7F, 0x3E, 0x36, 0x00],
            // 120 'x'
            [0x00, 0x00, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x00],
            // 121 'y'
            [0x00, 0x00, 0x66, 0x66, 0x66, 0x3E, 0x06, 0x3C],
            // 122 'z'
            [0x00, 0x00, 0x7E, 0x0C, 0x18, 0x30, 0x7E, 0x00],
            // 123 '{'
            [0x0E, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0E, 0x00],
            // 124 '|'
            [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
            // 125 '}'
            [0x70, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x70, 0x00],
            // 126 '~'
            [0x3E, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        let idx = c.wrapping_sub(32) as usize;
        if idx < 95 {
            &GLYPHS[idx]
        } else {
            &GLYPHS[0]
        }
    }

    pub fn draw_char(&mut self, x: u32, y: u32, c: char, fg: u32, bg: u32) {
        let glyph = Self::glyph(c as u8);
        for (row, &byte) in glyph.iter().enumerate() {
            for col in 0..8 {
                let px = x + col;
                let py = y + row as u32;
                if px < self.w && py < self.h {
                    if (byte >> (7 - col)) & 1 != 0 {
                        self.data[(py * self.w + px) as usize] = fg;
                    } else if bg != 0 {
                        self.data[(py * self.w + px) as usize] = bg;
                    }
                }
            }
        }
    }

    pub fn draw_string(&mut self, x: u32, y: u32, s: &str, fg: u32, bg: u32) {
        for (i, c) in s.chars().enumerate() {
            self.draw_char(x + i as u32 * 8, y, c, fg, bg);
        }
    }

    pub fn draw_shadow(&mut self, x: u32, y: u32, w: u32, h: u32, radius: u32, color: u32) {
        let color_alpha = (color >> 24) & 0xFF;
        if color_alpha == 0 || radius == 0 || w == 0 || h == 0 {
            return;
        }
        let rgb = color & 0x00FFFFFF;
        for i in 0..5 {
            let t = i as f32 / 4.0;
            let offset = 1 + ((radius.saturating_sub(1)) as f32 * t) as u32;
            let alpha = (color_alpha as f32 * (1.0 - t)) as u8;
            let col = rgb | ((alpha as u32) << 24);
            let sx = x.saturating_sub(offset);
            let sy = y.saturating_sub(offset);
            let sw = w + offset * 2;
            let sh = h + offset * 2;
            self.draw_rect_outline(sx, sy, sw, sh, col);
        }
    }
}

// ── LayerBuffer ─────────────────────────────────────────────────────────────

/// One compositor layer: an offscreen pixel buffer.
#[derive(Default)]
struct LayerBuffer {
    buf: Vec<u32>,
}

impl LayerBuffer {
    /// Allocate exactly `pixels` zeroed entries. Returns `Err(())` on OOM
    /// instead of panicking through the global alloc error handler.
    fn alloc(&mut self, pixels: usize) -> Result<(), ()> {
        self.buf.try_reserve_exact(pixels).map_err(|_| ())?;
        self.buf.resize(pixels, 0);
        Ok(())
    }

    fn clear(&mut self) {
        for px in self.buf.iter_mut() {
            *px = 0;
        }
    }
}

// ── Compositor ──────────────────────────────────────────────────────────────

pub struct Compositor {
    layers: [LayerBuffer; LAYER_COUNT],
    w: u32,
    h: u32,
    first_frame: bool,
}

impl Compositor {
    /// Returns `None` if any layer buffer allocation fails (OOM).
    pub fn new(w: u32, h: u32) -> Option<Self> {
        let pixels = (w * h) as usize;
        let mut layers: [LayerBuffer; LAYER_COUNT] = Default::default();
        for l in layers.iter_mut() {
            if l.alloc(pixels).is_err() {
                return None;
            }
        }
        Some(Compositor {
            layers,
            w,
            h,
            first_frame: true,
        })
    }

    /// Reset every layer buffer to transparent black.
    pub fn clear_all(&mut self) {
        for l in self.layers.iter_mut() {
            l.clear();
        }
    }

    /// Clear a single layer buffer.
    pub(crate) fn clear_layer(&mut self, layer: Layer) {
        self.layers[layer as usize].clear();
    }

    /// Return a [`Canvas`] that writes into the given layer's buffer.
    pub(crate) fn layer_canvas(&mut self, layer: Layer) -> Canvas<'_> {
        let buf = &mut self.layers[layer as usize].buf;
        Canvas {
            data: buf.as_mut_slice(),
            w: self.w,
            h: self.h,
        }
    }

    /// Composite all layers into the real window framebuffer in predefined
    /// draw order: wallpaper → desktop → windows → popups → overlay → cursor.
    ///
    /// Each layer is alpha-blended onto the output.  The cursor layer is
    /// drawn last (opaque) so it always sits on top.
    ///
    /// When `damage_rects` is `Some` and this is not the very first frame,
    /// only the given rectangular regions are composited — everything outside
    /// is left as-is.  The first frame always composites the full buffer.
    pub fn compose(&mut self, win: &mut libsarga::gui::Window, damage_rects: Option<&[Rect]>) {
        let dst = win.buffer_mut();
        let total = (self.w * self.h) as usize;
        let full = self.first_frame || damage_rects.is_none_or(|r| r.is_empty());
        self.first_frame = false;

        if full {
            // Start with wallpaper (full-opacity copy).
            dst.copy_from_slice(&self.layers[Layer::Wallpaper as usize].buf);

            // Blend each subsequent layer over the accumulated output.
            for li in 1..LAYER_COUNT {
                let src = &self.layers[li].buf;
                for i in 0..total {
                    let px = src[i];
                    if px & 0xFF000000 == 0 {
                        continue;
                    }
                    if px >> 24 == 0xFF {
                        dst[i] = px;
                    } else {
                        dst[i] = alpha_blend(dst[i], px, (px >> 24) as u8);
                    }
                }
            }
            return;
        }

        // Partial compose — only touch the damaged rects.
        if let Some(rects) = damage_rects {
            let sw = self.w as usize;
            for r in rects {
                let x0 = r.x.max(0) as usize;
                let y0 = r.y.max(0) as usize;
                let x1 = ((r.x + r.w as i32).min(self.w as i32)).max(0) as usize;
                let y1 = ((r.y + r.h as i32).min(self.h as i32)).max(0) as usize;
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let rw_px = x1 - x0;

                // Copy wallpaper rect.
                let wallpaper = &self.layers[Layer::Wallpaper as usize].buf;
                for py in y0..y1 {
                    let off = py * sw + x0;
                    dst[off..off + rw_px].copy_from_slice(&wallpaper[off..off + rw_px]);
                }

                // Blend each subsequent layer over this rect.
                for li in 1..LAYER_COUNT {
                    let src = &self.layers[li].buf;
                    for py in y0..y1 {
                        let row_off = py * sw;
                        for px in x0..x1 {
                            let i = row_off + px;
                            let px_val = src[i];
                            if px_val & 0xFF000000 == 0 {
                                continue;
                            }
                            if px_val >> 24 == 0xFF {
                                dst[i] = px_val;
                            } else {
                                dst[i] = alpha_blend(dst[i], px_val, (px_val >> 24) as u8);
                            }
                        }
                    }
                }
            }
        }
    }
}
