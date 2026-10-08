#[derive(Clone, Copy)]
pub enum Key {
    Character(u8),
    Enter,
    Backspace,
    Escape,
    Tab,
    View(usize),
}
pub struct Keyboard {
    shift: u8,
    extended: bool,
}
impl Keyboard {
    pub const fn new() -> Self {
        Self {
            shift: 0,
            extended: false,
        }
    }
    /// A lost release or extended prefix must not leave a modifier latched.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub fn decode(&mut self, code: u8) -> Option<Key> {
        if code == 0xe0 {
            self.extended = true;
            return None;
        }
        if self.extended {
            self.extended = false;
            return None;
        }
        if code == 0x2a || code == 0x36 {
            self.shift |= if code == 0x2a { 1 } else { 2 };
            return None;
        }
        if code == 0xaa || code == 0xb6 {
            self.shift &= !(if code == 0xaa { 1 } else { 2 });
            return None;
        }
        if code & 0x80 != 0 {
            return None;
        }
        match code {
            0x01 => return Some(Key::Escape),
            0x0e => return Some(Key::Backspace),
            0x1c => return Some(Key::Enter),
            0x0f => return Some(Key::Tab),
            0x3b..=0x3f => return Some(Key::View((code - 0x3b) as usize)),
            _ => {}
        }
        let byte = match code {
            0x10..=0x19 => b"qwertyuiop"[(code - 0x10) as usize],
            0x1e..=0x26 => b"asdfghjkl"[(code - 0x1e) as usize],
            0x2c..=0x32 => b"zxcvbnm"[(code - 0x2c) as usize],
            0x02..=0x0b => b"1234567890"[(code - 0x02) as usize],
            0x0c => b'-',
            0x0d => b'=',
            0x1a => b'[',
            0x1b => b']',
            0x27 => b';',
            0x28 => b'\'',
            0x29 => b'`',
            0x2b => b'\\',
            0x33 => b',',
            0x34 => b'.',
            0x35 => b'/',
            0x39 => b' ',
            _ => 0,
        };
        if byte == 0 {
            return None;
        }
        let shifted = if self.shift != 0 {
            match byte {
                b'1' => b'!',
                b'2' => b'@',
                b'3' => b'#',
                b'4' => b'$',
                b'5' => b'%',
                b'6' => b'^',
                b'7' => b'&',
                b'8' => b'*',
                b'9' => b'(',
                b'0' => b')',
                b'-' => b'_',
                b'=' => b'+',
                b'[' => b'{',
                b']' => b'}',
                b';' => b':',
                b'\'' => b'"',
                b'`' => b'~',
                b'\\' => b'|',
                b',' => b'<',
                b'.' => b'>',
                b'/' => b'?',
                other => other.to_ascii_uppercase(),
            }
        } else {
            byte
        };
        Some(Key::Character(shifted))
    }
}

/// Assemble bytes before enqueueing. Overflow always discards a whole packet,
/// so displacement bytes can never become headers when the consumer resumes.
pub struct MousePackets {
    packet: [u8; 3],
    index: usize,
    queued: [[u8; 3]; 64],
    head: usize,
    tail: usize,
    dropped: u64,
}
impl Default for MousePackets {
    fn default() -> Self {
        Self::new()
    }
}
impl MousePackets {
    pub const fn new() -> Self {
        Self {
            packet: [0; 3],
            index: 0,
            queued: [[0; 3]; 64],
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }
    pub fn push_byte(&mut self, byte: u8) {
        if self.index == 0 && byte & 8 == 0 {
            return;
        }
        self.packet[self.index] = byte;
        self.index += 1;
        if self.index != 3 {
            return;
        }
        self.index = 0;
        let next = (self.head + 1) & 63;
        if next == self.tail {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.queued[self.head] = self.packet;
        self.head = next;
    }
    pub fn pop(&mut self) -> Option<[u8; 3]> {
        if self.tail == self.head {
            return None;
        }
        let packet = self.queued[self.tail];
        self.tail = (self.tail + 1) & 63;
        Some(packet)
    }
    pub fn dropped_packets(&self) -> u64 {
        self.dropped
    }
}

pub struct Mouse {
    pub x: i32,
    pub y: i32,
    pressed: bool,
}
impl Mouse {
    pub const fn new() -> Self {
        Self {
            x: 500,
            y: 380,
            pressed: false,
        }
    }
    pub fn decode(&mut self, packet: [u8; 3], width: usize, height: usize) -> Option<bool> {
        if packet[0] & 8 == 0 || packet[0] & 0xc0 != 0 {
            return None;
        }
        let dx = packet[1] as i32 - if packet[0] & 0x10 != 0 { 256 } else { 0 };
        let dy = packet[2] as i32 - if packet[0] & 0x20 != 0 { 256 } else { 0 };
        self.x = (self.x + dx).clamp(0, width.saturating_sub(16) as i32);
        self.y = (self.y - dy).clamp(0, height.saturating_sub(16) as i32);
        let pressed = packet[0] & 1 != 0;
        let clicked = pressed && !self.pressed;
        self.pressed = pressed;
        Some(clicked)
    }
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for Mouse {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflowing_mouse_queue_keeps_packet_boundaries() {
        let mut packets = MousePackets::new();
        let mut mouse = Mouse::new();
        for _ in 0..63 {
            for byte in [8, 8, 0] {
                packets.push_byte(byte);
            }
        }
        // Start a packet while full, then drain before its last byte. It is
        // either accepted intact or dropped intact, even across this boundary.
        packets.push_byte(8);
        packets.push_byte(8);
        for _ in 0..63 {
            mouse.decode(packets.pop().unwrap(), 4096, 768);
        }
        packets.push_byte(0);
        mouse.decode(packets.pop().unwrap(), 4096, 768);
        for _ in 0..64 {
            for byte in [8, 8, 0] {
                packets.push_byte(byte);
            }
        }
        assert_eq!(packets.dropped_packets(), 1);
        while let Some(packet) = packets.pop() {
            mouse.decode(packet, 4096, 768);
        }
        for _ in 0..30 {
            for byte in [8, 8, 0] {
                packets.push_byte(byte);
            }
            mouse.decode(packets.pop().unwrap(), 4096, 768);
        }
        assert_eq!((mouse.x, mouse.y), (500 + (63 + 1 + 63 + 30) * 8, 380));
    }
    #[test]
    fn mouse_buttons_are_edges_and_overflow_packets_are_ignored() {
        let mut mouse = Mouse::new();
        assert_eq!(mouse.decode([9, 0, 0], 1024, 768), Some(true));
        assert_eq!(mouse.decode([9, 0, 0], 1024, 768), Some(false));
        assert_eq!(mouse.decode([0xc8, 100, 100], 1024, 768), None);
        mouse.decode([8, 0, 0], 1024, 768);
        assert_eq!(mouse.decode([9, 0, 0], 1024, 768), Some(true));
    }
    #[test]
    fn releasing_one_shift_key_keeps_the_other_pressed() {
        let mut keyboard = Keyboard::new();
        for code in [0x2a, 0x36, 0xaa] {
            keyboard.decode(code);
        }
        assert!(matches!(keyboard.decode(0x1e), Some(Key::Character(b'A'))));
    }
    #[test]
    fn keyboard_overflow_reset_clears_modifiers_and_extended_prefix() {
        let mut keyboard = Keyboard::new();
        keyboard.decode(0x2a);
        keyboard.decode(0xe0);
        keyboard.reset();
        assert!(matches!(keyboard.decode(0x1e), Some(Key::Character(b'a'))));
    }
}
