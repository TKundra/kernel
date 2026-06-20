//! # VGA text-mode writer (our screen "driver")
//!
//! On a PC booted in VGA text mode, the screen is a grid of 80x25 cells living
//! at the fixed physical address `0xb8000`. Each cell is two bytes:
//!
//!   byte 0: the ASCII character to display
//!   byte 1: the color   ->  high nibble = background, low nibble = foreground
//!
//! Writing a byte pair into that memory makes a character appear instantly —
//! no operating system involved. This module wraps that raw memory in a small
//! `Writer` and exposes `print!` / `println!` macros so the rest of the kernel
//! can format text just like it would with `std`.

use core::fmt;

use lazy_static::lazy_static;
use spin::Mutex;
use volatile::Volatile;

/// The 16 colors VGA text mode supports. `#[repr(u8)]` makes each variant fit
/// in the 4-bit color nibble.
#[allow(dead_code)] // not every color is used, but we list them for reference
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// A full color byte: background in the high nibble, foreground in the low one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

/// One cell on screen: a character plus its color. `#[repr(C)]` guarantees the
/// field order in memory (char first, color second), matching the hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

/// The screen itself, laid out exactly as the hardware expects. Each cell is
/// wrapped in `Volatile` so the compiler never optimizes our writes away.
#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// Tracks where we are on screen and writes characters into the buffer.
///
/// We always write to the bottom row and scroll the whole screen up when it
/// fills — the simplest behavior that feels like a real terminal.
pub struct Writer {
    column_position: usize,
    color_code: ColorCode,
    buffer: &'static mut Buffer,
}

impl Writer {
    /// Write a single byte, handling newlines and line wrapping.
    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                // Wrap to the next line if the current one is full.
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1; // always the bottom row
                let col = self.column_position;
                self.buffer.chars[row][col].write(ScreenChar {
                    ascii_character: byte,
                    color_code: self.color_code,
                });
                self.column_position += 1;
            }
        }
    }

    /// Write a string, substituting a `■` for any non-printable byte (the VGA
    /// font only has the printable ASCII range plus some extras).
    fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                // printable ASCII or newline
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                // anything else
                _ => self.write_byte(0xfe),
            }
        }
    }

    /// Move to a fresh line: scroll every row up by one, then clear the bottom.
    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = self.buffer.chars[row][col].read();
                self.buffer.chars[row - 1][col].write(character);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    /// Overwrite a whole row with blank spaces.
    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }

    /// Erase the character just before the cursor (used by the shell's
    /// Backspace handling). Does nothing at the start of a line.
    fn backspace(&mut self) {
        if self.column_position > 0 {
            self.column_position -= 1;
            let row = BUFFER_HEIGHT - 1;
            let col = self.column_position;
            self.buffer.chars[row][col].write(ScreenChar {
                ascii_character: b' ',
                color_code: self.color_code,
            });
        }
    }
}

/// This lets us use Rust's formatting machinery (`write!`, `{}` placeholders)
/// with our writer, which is what powers the `print!` macro below.
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

lazy_static! {
    /// The one global writer, behind a spinlock so it can be shared safely.
    /// Casting `0xb8000` to a `&mut Buffer` is the unsafe step that ties our
    /// type to the real hardware memory.
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: ColorCode::new(Color::Yellow, Color::Black),
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}

// ---- Public macros ------------------------------------------------------
// These mirror the standard library's `print!`/`println!` so the rest of the
// kernel reads naturally.

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Implementation behind the macros. We disable interrupts while holding the
/// writer lock so that a keyboard interrupt (which also prints) can never fire
/// mid-print and deadlock on the same lock.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
    });
}

// ---- Screen helpers used by shell commands ------------------------------

/// Clear the entire screen and move the cursor home (the `clear` command).
pub fn clear_screen() {
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        let mut writer = WRITER.lock();
        for row in 0..BUFFER_HEIGHT {
            writer.clear_row(row);
        }
        writer.column_position = 0;
    });
}

/// Erase the previous character on screen (the shell's Backspace handling).
pub fn backspace() {
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        WRITER.lock().backspace();
    });
}

/// Switch to a white-on-red color, used by the panic handler so a kernel panic
/// is impossible to miss.
pub fn set_panic_color() {
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        WRITER.lock().color_code = ColorCode::new(Color::White, Color::Red);
    });
}

/// Print `s` in the given foreground/background, then restore the previous
/// color. Used by the `colors` command to show off the VGA palette without
/// permanently changing the shell's text color.
pub fn print_colored(s: &str, foreground: Color, background: Color) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        let mut writer = WRITER.lock();
        let saved = writer.color_code;
        writer.color_code = ColorCode::new(foreground, background);
        // `write_str` can't actually fail for our writer.
        let _ = writer.write_str(s);
        writer.color_code = saved;
    });
}
