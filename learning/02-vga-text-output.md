# Chapter 2 — Printing to the screen (VGA text mode)

**Real file:** `../src/vga_buffer.rs`
**Goal:** make text appear on screen with no OS, and rebuild `print!`/`println!`.

🎯 **Milestone:** you understand that "printing" is just writing two bytes per
character into memory at `0xb8000` — and you'll read those exact bytes back live
with the `peek` command.

---

## The big idea: the screen is just memory

When a PC boots in VGA text mode, the screen is an **80×25 grid** of character
cells, and that grid lives at a fixed physical address: **`0xb8000`**. Each cell
is **two bytes**:

```
   one cell (2 bytes):
   +------------------+---------------------------------+
   | byte 0: ASCII    | byte 1: color                   |
   |  character code  |   high nibble = background      |
   |                  |   low  nibble = foreground      |
   +------------------+---------------------------------+
```

Write those two bytes and a character appears instantly. No driver, no syscall.
"Print a character" literally means "store two bytes in memory".

---

## Describing a color

```rust
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0, Blue = 1, Green = 2, Cyan = 3,
    Red = 4, Magenta = 5, Brown = 6, LightGray = 7,
    DarkGray = 8, LightBlue = 9, LightGreen = 10, LightCyan = 11,
    LightRed = 12, Pink = 13, Yellow = 14, White = 15,
}
```

- `#[repr(u8)]` forces each variant to be a single byte, so it fits the 4-bit
  color nibble.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}
```

- `ColorCode` packs background and foreground into one byte: background shifted
  into the high 4 bits, foreground in the low 4 bits.
- `#[repr(transparent)]` means a `ColorCode` is laid out exactly like the `u8`
  inside it — no extra bytes.

---

## Describing a cell and the whole screen

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}
```

- `#[repr(C)]` guarantees the fields stay in this order in memory (char first,
  color second) — which is exactly what the hardware expects.

```rust
const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}
```

- `Buffer` is the entire screen: 25 rows × 80 columns of cells.
- Each cell is wrapped in **`Volatile`**. Why? The compiler can't see that this
  memory is special hardware. It might "optimize away" writes it thinks are
  pointless (we write but never read back). `Volatile::write` tells the compiler
  *"really do this memory access, don't optimize it"*.

---

## The writer

```rust
pub struct Writer {
    column_position: usize,        // where the next char goes on the bottom row
    color_code: ColorCode,         // current color
    buffer: &'static mut Buffer,   // the screen memory
}
```

Our strategy: always write to the **bottom row**, and when a newline comes,
scroll everything up by one row. Simple, and it feels like a terminal.

Writing one byte:

```rust
fn write_byte(&mut self, byte: u8) {
    match byte {
        b'\n' => self.new_line(),
        byte => {
            if self.column_position >= BUFFER_WIDTH {
                self.new_line();             // wrap when the row is full
            }
            let row = BUFFER_HEIGHT - 1;     // bottom row
            let col = self.column_position;
            self.buffer.chars[row][col].write(ScreenChar {
                ascii_character: byte,
                color_code: self.color_code,
            });
            self.column_position += 1;
        }
    }
}
```

Scrolling on a newline — copy each row up into the one above, clear the bottom:

```rust
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
```

Erasing a character (used by Backspace later) — step back, write a space:

```rust
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
```

---

## Hooking into Rust's formatting (`{}` placeholders)

To get `print!("{}", x)` for free, we implement `core::fmt::Write`:

```rust
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}
```

Now Rust's formatting machinery can drive our writer. One method, and all of
`{}`, `{:#x}`, `{:?}` formatting works.

---

## One global writer (behind a lock)

```rust
lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: ColorCode::new(Color::Yellow, Color::Black),
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}
```

- `0xb8000 as *mut Buffer` is the one **`unsafe`** line that ties our nice Rust
  type to the real hardware address. We promise the compiler that this address
  really is a screen buffer.
- `lazy_static!` lets us build a `static` that needs a tiny bit of runtime setup.
- `Mutex` (a **spinlock** from the `spin` crate) lets the writer be shared
  safely. We can't use `std::sync::Mutex` — it would try to *sleep* a thread,
  and we have no threads or scheduler. A spinlock just spins, which is right for
  a kernel.

---

## The `print!` / `println!` macros

```rust
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
    });
}
```

These mirror the standard library's macros, so the rest of the kernel reads
naturally. The important detail is in `_print`:

- **`without_interrupts(...)`** disables interrupts while we hold the writer
  lock. Why? The keyboard interrupt *also* prints (it echoes keys). If a
  keyboard interrupt fired while we held the lock, its handler would spin
  forever waiting for a lock that can't be released — a **deadlock**. Turning
  interrupts off for the brief moment we hold the lock makes that impossible.

---

## Helpers used by shell commands

```rust
pub fn clear_screen() { /* clear every row, cursor to top */ }
pub fn backspace()    { /* erase the previous character */ }
pub fn set_panic_color() {
    // white-on-red, so a kernel panic is impossible to miss
}
```

Each wraps the locked writer in `without_interrupts` just like `_print`.

---

## ✅ Checkpoint — printing is just memory

Run the kernel and prove the screen really is memory:

```bash
cargo run
```

1. The yellow banner and `kernel>` prompt you see *are* this code running —
   every character is two bytes written at `0xb8000`.
2. Clear the screen (this writes blank cells to every position):
   ```
   kernel> clear
   ```
3. Now read the screen's own video memory back as raw bytes:
   ```
   kernel> peek 0xb8000 32
   ```
   You'll see pairs like `20 0e` repeating — `0x20` is the ASCII space and
   `0x0e` is the color byte (yellow-on-black: bg `0`, fg `0xe`). **That second
   byte is the `ColorCode` from this chapter, sitting in real memory.**
4. Now **write** to that memory and watch the screen change instantly:
   ```
   kernel> poke 0xb8000 0x41
   ```
   An `A` appears in the top-left corner — you just stored one byte. And see the
   whole 16-color palette this chapter describes:
   ```
   kernel> colors
   ```

> Experiment: open `src/vga_buffer.rs`, find the `WRITER` color
> (`Color::Yellow, Color::Black`), change `Yellow` to `LightGreen`, then
> `cargo run` again. The whole shell is now green. Change it back when done.

---

## What you learned

- The screen is memory at `0xb8000`; printing = writing two bytes per cell.
- `Volatile` stops the compiler from optimizing away hardware writes.
- Implementing `fmt::Write` gives us `{}` formatting; the macros build on it.
- A global behind a **spinlock**, with **interrupts disabled** while locked,
  is the kernel-correct way to share the writer.

**Next:** [Chapter 3 — A safety net: GDT + TSS](03-gdt-and-tss.md).
