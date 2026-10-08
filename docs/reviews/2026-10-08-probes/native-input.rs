#[path = "../../../baremetal/src/input.rs"]
mod input;

fn main() {
    let mut mouse = input::Mouse::new();
    let packet = [0x08, 0x08, 0x00]; // no buttons, move right by 8
    for _ in 0..21 { mouse.decode(packet, 1024, 768); }
    let start = (mouse.x, mouse.y);
    for _ in 0..30 { mouse.decode(packet, 1024, 768); }
    println!("after queue: {:?}; after 30 right-only packets: ({}, {})", start, mouse.x, mouse.y);
    let mut keyboard = input::Keyboard::new();
    keyboard.decode(0x2a); // left shift down
    keyboard.decode(0x36); // right shift down
    keyboard.decode(0xaa); // left shift up, right shift still held
    if let Some(input::Key::Character(byte)) = keyboard.decode(0x1e) { println!("both Shift keys, then release left and type a: {}", byte as char); }
}
