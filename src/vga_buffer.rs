//! # VGA text-mode writer (minimal kernel console)
//!
//! In legacy PC VGA text mode, the screen is a fixed 80×25 grid mapped directly
//! to physical memory at `0xb8000`.
//!
//! Each screen cell is exactly 2 bytes:
//!
//! - byte 0: ASCII character to display
//! - byte 1: color attribute
//!   - high 4 bits → background color
//!   - low 4 bits  → foreground color
//!
//! Because this memory is directly mapped to the display hardware, writing
//! values here immediately updates what appears on screen—no OS or drivers
//! involved.
//!
//! This module wraps that raw memory into a safe-ish `Writer` abstraction and
//! provides `print!` / `println!` macros so the rest of the kernel can use
//! Rust-style formatting.

use volatile::Volatile;
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;

/// VGA supports a fixed 16-color palette.
/// `#[repr(u8)]` ensures each variant fits into 4 bits of the color byte.
#[allow(dead_code)] // kept for completeness even if not all colors are used
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

/// Encodes a VGA color attribute byte.
///
/// Layout:
/// - bits 7–4: background color
/// - bits 3–0: foreground color
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    /// Create a color attribute from foreground and background colors.
    fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

/// A single VGA screen cell.
///
/// The memory layout must match VGA expectations exactly:
/// character byte first, then color byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

/// VGA text buffer layout in memory.
///
/// Each cell is wrapped in `Volatile` to prevent the compiler from
/// optimizing away writes (since this is memory-mapped hardware).
#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// A simple text writer for VGA mode.
///
/// This always writes to the last row of the screen. When the row fills,
/// the screen scrolls upward by one line.
///
/// This mimics a basic terminal behavior.
pub struct Writer {
    column_position: usize,       // next write position on the bottom row
    color_code: ColorCode,        // active foreground/background colors
    buffer: &'static mut Buffer,  // memory-mapped VGA buffer
}

impl Writer {
    /// Writes a single byte to the screen.
    ///
    /// Special handling:
    /// - `\n` triggers a line break (scrolling)
    /// - other bytes are written at the current cursor position
    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                // Wrap to next line if we reach the end of the row
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1; // always write on bottom row
                let col = self.column_position;

                self.buffer.chars[row][col].write(ScreenChar {
                    ascii_character: byte,
                    color_code: self.color_code,
                });

                self.column_position += 1;
            }
        }
    }

    /// Scroll the screen up by one row.
    ///
    /// Implementation:
    /// - Copy each row into the one above it
    /// - Clear the last row
    /// - Reset cursor to column 0
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

    /// Fill a row with blank spaces using the current color.
    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };

        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }

    /// Write a UTF-8 string to the screen.
    ///
    /// VGA text mode only supports extended ASCII, so:
    /// - printable ASCII characters are written directly
    /// - everything else is replaced with `0xFE` (■ block character)
    fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
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

/// Allows integration with Rust formatting macros (`write!`, `{}` formatting).
///
/// This is what enables higher-level `print!` / `println!` support.
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

// Global VGA writer (protected by a mutex)
//
// We expose a single shared writer for the whole kernel.
// Since multiple contexts (main code + interrupts) may print,
// access must be synchronized to avoid race conditions.
lazy_static! {
    /// Global VGA text writer protected by a spinlock mutex.
    ///
    /// SAFETY NOTE:
    /// We manually map the VGA buffer at `0xb8000` by casting a raw pointer
    /// into a `&mut Buffer`. This is safe only because:
    /// - VGA memory is always present in standard PC hardware
    /// - we guarantee exclusive mutable access through `Mutex`
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: ColorCode::new(Color::Yellow, Color::Black),
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}

// -------------------------------------------------------------------------
// Public printing macros
// -------------------------------------------------------------------------
//
// These replicate `std::print!` and `std::println!` so kernel code can use
// familiar formatting syntax.

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Internal printing backend used by `print!` / `println!`.
///
/// This function ensures two critical properties:
/// 1. The writer is accessed with mutual exclusion (via the mutex)
/// 2. Interrupts are disabled while holding the lock
///
/// WHY INTERRUPTS ARE DISABLED:
/// A hardware interrupt (e.g. keyboard input) may also attempt to print.
/// If it fires while we already hold the lock, it would try to lock again
/// in the same context, causing a deadlock.
///
/// Disabling interrupts guarantees that printing is atomic with respect to
/// interrupt handlers on the same CPU.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // Prevent interrupt handlers from interrupting this critical section
    interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
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

// -------------------------------------------------------------------------
// Screen helpers used by shell commands
// -------------------------------------------------------------------------

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