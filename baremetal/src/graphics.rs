//! 直接绘制 GOP 提供的物理帧缓冲；固件退出后不使用 GOP 方法。
pub struct Framebuffer {
    base: *mut u32,
    pub width: usize,
    pub height: usize,
    stride: usize,
    rgb: bool,
}
impl Framebuffer {
    pub const fn new(base: usize, width: usize, height: usize, stride: usize, rgb: bool) -> Self {
        Self {
            base: base as *mut u32,
            width,
            height,
            stride,
            rgb,
        }
    }
    fn color(&self, color: u32) -> u32 {
        if self.rgb {
            (color & 0xff00) | ((color >> 16) & 0xff) | ((color & 0xff) << 16)
        } else {
            color
        }
    }
    pub fn pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            unsafe {
                self.base
                    .add(y * self.stride + x)
                    .write_volatile(self.color(color));
            }
        }
    }
    pub fn rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: u32) {
        let color = self.color(color);
        for py in y..y.saturating_add(height).min(self.height) {
            for px in x..x.saturating_add(width).min(self.width) {
                unsafe {
                    self.base.add(py * self.stride + px).write_volatile(color);
                }
            }
        }
    }
    pub fn text(&mut self, mut x: usize, mut y: usize, text: &str, color: u32, scale: usize) {
        let origin = x;
        for byte in text.bytes() {
            if byte == b'\n' {
                x = origin;
                y += 9 * scale;
                continue;
            }
            if x + 5 * scale >= self.width {
                break;
            }
            let glyph = glyph(byte.to_ascii_uppercase());
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect(x + col * scale, y + row * scale, scale, scale, color);
                    }
                }
            }
            x += 6 * scale;
        }
    }
    pub fn cursor(&mut self, x: usize, y: usize) {
        for dy in 0..16 {
            for dx in 0..=(dy / 2).min(7) {
                self.pixel(x + dx + 1, y + dy + 1, 0x172b4d);
                self.pixel(x + dx, y + dy, 0xffffff);
            }
        }
    }
}

const CURSOR_WIDTH: usize = 9;
const CURSOR_HEIGHT: usize = 17;
/// Save the small rectangle covered by the cursor rather than repainting the
/// desktop on every movement. Stored colors are raw framebuffer pixels.
pub struct Cursor {
    position: Option<(usize, usize)>,
    background: [u32; CURSOR_WIDTH * CURSOR_HEIGHT],
}
impl Default for Cursor {
    fn default() -> Self {
        Self::new()
    }
}
impl Cursor {
    pub const fn new() -> Self {
        Self {
            position: None,
            background: [0; CURSOR_WIDTH * CURSOR_HEIGHT],
        }
    }
    pub fn hide(&mut self, fb: &mut Framebuffer) {
        if let Some((x, y)) = self.position.take() {
            for dy in 0..CURSOR_HEIGHT.min(fb.height.saturating_sub(y)) {
                for dx in 0..CURSOR_WIDTH.min(fb.width.saturating_sub(x)) {
                    unsafe {
                        fb.base
                            .add((y + dy) * fb.stride + x + dx)
                            .write_volatile(self.background[dy * CURSOR_WIDTH + dx]);
                    }
                }
            }
        }
    }
    pub fn show(&mut self, fb: &mut Framebuffer, x: usize, y: usize) {
        if self.position == Some((x, y)) {
            return;
        }
        self.hide(fb);
        for dy in 0..CURSOR_HEIGHT.min(fb.height.saturating_sub(y)) {
            for dx in 0..CURSOR_WIDTH.min(fb.width.saturating_sub(x)) {
                self.background[dy * CURSOR_WIDTH + dx] =
                    unsafe { fb.base.add((y + dy) * fb.stride + x + dx).read_volatile() };
            }
        }
        self.position = Some((x, y));
        fb.cursor(x, y);
    }
}

fn glyph(byte: u8) -> [u8; 7] {
    match byte {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'J' => [7, 2, 2, 2, 2, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 25, 21, 19, 19, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 27, 17],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b'.' => [0, 0, 0, 0, 0, 12, 12],
        b',' => [0, 0, 0, 0, 0, 4, 8],
        b':' => [0, 12, 12, 0, 12, 12, 0],
        b';' => [0, 12, 12, 0, 4, 4, 8],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'_' => [0, 0, 0, 0, 0, 0, 31],
        b'/' => [1, 2, 2, 4, 8, 8, 16],
        b'\\' => [16, 8, 8, 4, 2, 2, 1],
        b'>' => [16, 8, 4, 2, 4, 8, 16],
        b'<' => [1, 2, 4, 8, 4, 2, 1],
        b'=' => [0, 31, 0, 31, 0, 0, 0],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        b'[' => [14, 8, 8, 8, 8, 8, 14],
        b']' => [14, 2, 2, 2, 2, 2, 14],
        b'(' => [2, 4, 8, 8, 8, 4, 2],
        b')' => [8, 4, 2, 2, 2, 4, 8],
        b'%' => [17, 2, 4, 4, 8, 17, 0],
        b'!' => [4, 4, 4, 4, 4, 0, 4],
        b'?' => [14, 17, 1, 2, 4, 0, 4],
        b'#' => [10, 31, 10, 10, 31, 10, 0],
        b'"' => [10, 10, 10, 0, 0, 0, 0],
        b'\'' => [4, 4, 8, 0, 0, 0, 0],
        b'*' => [0, 21, 14, 31, 14, 21, 0],
        b' ' => [0; 7],
        _ => [31, 17, 5, 4, 4, 0, 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_restores_overlapping_moves_and_clipped_edges_in_both_formats() {
        for rgb in [false, true] {
            let mut pixels = [0u32; 40 * 32];
            for (index, pixel) in pixels.iter_mut().enumerate() {
                *pixel = index as u32 * 0x010203;
            }
            let original = pixels;
            let mut fb = Framebuffer::new(pixels.as_mut_ptr() as usize, 32, 32, 40, rgb);
            let mut cursor = Cursor::new();
            for (x, y) in [(2, 2), (3, 3), (30, 30), (0, 0)] {
                cursor.show(&mut fb, x, y);
            }
            cursor.hide(&mut fb);
            assert_eq!(pixels, original);
            // A full content draw must refresh the saved background.
            cursor.show(&mut fb, 4, 4);
            cursor.hide(&mut fb);
            fb.rect(0, 0, 32, 32, 0xabcdef);
            let replacement = pixels;
            cursor.show(&mut fb, 4, 4);
            cursor.hide(&mut fb);
            assert_eq!(pixels, replacement);
        }
    }
}
