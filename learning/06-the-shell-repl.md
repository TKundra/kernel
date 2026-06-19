# Chapter 6 — The shell (REPL + line editing)

**Real file:** `../src/shell.rs` (the structure + line editing)
**Goal:** turn the stream of scancodes into edited lines of text, and run the
matching command. This is the "bottom half" from Chapter 5.

---

## The REPL loop (in `main.rs`)

First, recall how the shell is driven. `kernel_main` ends with:

```rust
let mut shell = shell::Shell::new(&boot_info.memory_map);
shell.start();

loop {
    match interrupts::next_scancode() {
        Some(scancode) => shell.feed_scancode(scancode),
        None => x86_64::instructions::interrupts::enable_and_hlt(),
    }
}
```

- When there's a scancode, feed it to the shell.
- When there isn't, **`enable_and_hlt()`** atomically enables interrupts and
  halts the CPU. The CPU sleeps until the next interrupt (a keypress), instead
  of spinning hot and wasting power. The "atomically" matters: it closes the
  tiny window where an interrupt could arrive between "check empty" and "halt".

This is the **Loop** in Read-Eval-Print-Loop.

---

## The shell's state

```rust
const MAX_LINE: usize = 128;

pub struct Shell {
    keyboard: Keyboard<layouts::Us104Key, ScancodeSet1>,  // scancode decoder
    line: [u8; MAX_LINE],                                  // the line buffer
    len: usize,                                            // chars used so far
    memory_map: &'static MemoryMap,                        // for the `mem` cmd
}
```

- No `String` (no heap!), so the line buffer is a fixed `[u8; 128]` array plus a
  length. Type past 128 chars and the extra is dropped.
- `Keyboard<...>` comes from the `pc-keyboard` crate — it knows how to turn raw
  scancodes into keys for a US layout.

```rust
pub fn new(memory_map: &'static MemoryMap) -> Self {
    Shell {
        keyboard: Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore,   // pass Ctrl+key through as normal keys
        ),
        line: [0; MAX_LINE],
        len: 0,
        memory_map,
    }
}
```

The banner + first prompt:

```rust
pub fn start(&self) {
    println!("=============================================");
    println!("  mini kernel shell  -  type 'help' to begin");
    println!("=============================================");
    self.prompt();
}

fn prompt(&self) {
    print!("kernel> ");
}
```

---

## Decoding one scancode

```rust
pub fn feed_scancode(&mut self, scancode: u8) {
    if let Ok(Some(key_event)) = self.keyboard.add_byte(scancode) {
        if let Some(key) = self.keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(character) => self.on_char(character),
                DecodedKey::RawKey(_) => {}   // arrows, F-keys: ignored
            }
        }
    }
}
```

Decoding is two steps because keys can be multi-byte and there are press *and*
release events:

- `add_byte` assembles bytes and yields a `KeyEvent` once a full event is ready.
- `process_keyevent` turns that into an actual character (applying Shift, etc.),
  or `None` for things like a key *release*.
- A `DecodedKey::Unicode('h')` is a real character; a `RawKey` is a non-text key
  we ignore for now.

---

## Line discipline: what to do with each character

```rust
fn on_char(&mut self, character: char) {
    match character {
        '\n' => self.on_enter(),         // Enter: run the line
        '\u{8}' => self.on_backspace(),  // Backspace (ASCII 0x08)
        c if c.is_ascii() && !c.is_ascii_control() => self.push_char(c),
        _ => {}                          // ignore other control/non-ASCII
    }
}
```

Appending a normal character — store it **and echo it** so the user sees it:

```rust
fn push_char(&mut self, character: char) {
    if self.len < MAX_LINE {
        self.line[self.len] = character as u8;
        self.len += 1;
        print!("{}", character);
    }
}
```

Backspace — shrink the buffer and erase on screen:

```rust
fn on_backspace(&mut self) {
    if self.len > 0 {
        self.len -= 1;
        vga_buffer::backspace();   // the helper from Chapter 2
    }
}
```

> "Echoing" is something the line discipline does itself. In raw keyboard input
> there's no automatic echo — if we didn't `print!` the character, typing would
> be invisible.

---

## Enter: finish the line and run it

```rust
fn on_enter(&mut self) {
    println!();   // move to the next screen line

    let input = core::str::from_utf8(&self.line[..self.len])
        .unwrap_or("")
        .trim();

    self.dispatch(input);

    self.len = 0;   // reset the buffer
    self.prompt();  // show a fresh prompt
}
```

- `from_utf8(&self.line[..self.len])` reinterprets the raw bytes as a string
  slice. Keyboard input is ASCII, so this always succeeds; `unwrap_or("")` is a
  safety fallback.
- `.trim()` removes stray surrounding whitespace.

---

## Parsing and dispatch

```rust
fn dispatch(&self, input: &str) {
    if input.is_empty() { return; }

    let mut parts = input.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim_start();

    match command {
        "help"     => self.cmd_help(),
        "clear"    => vga_buffer::clear_screen(),
        "echo"     => println!("{}", args),
        "mem"      => self.cmd_mem(),
        "regs"     => self.cmd_regs(),
        "cpuid"    => self.cmd_cpuid(),
        "uptime"   => self.cmd_uptime(),
        "int3"     => self.cmd_int3(),
        "peek"     => self.cmd_peek(args),
        "reboot"   => self.cmd_reboot(),
        "shutdown" => self.cmd_shutdown(),
        "panic"    => self.cmd_panic(args),
        other      => println!("unknown command: '{}'  (type 'help')", other),
    }
}
```

- `splitn(2, char::is_whitespace)` splits on the **first** whitespace only, into
  at most two pieces: the command word, and "everything else" (the arguments).
  That's why `echo hello   world` keeps its inner spacing.
- The `match` is the dispatch table. Adding a command = adding one arm here plus
  a handler method (Chapter 7).

---

## The full keystroke → command flow

```
   scancode -> feed_scancode -> decode -> on_char
                                            |
              +-----------------------------+-----------------------------+
              |                             |                             |
          printable                     Backspace                       Enter
        push_char + echo            shrink + erase             from_utf8 -> trim
                                                                   -> dispatch
                                                                   -> reset + prompt
```

---

## What you learned

- The main loop sleeps with `enable_and_hlt` and wakes on keypress.
- `pc-keyboard` decodes scancodes; we handle the resulting characters ourselves.
- Line discipline = building a fixed-size buffer, echoing, and handling
  Backspace/Enter.
- `splitn(2, …)` cleanly separates the command from its arguments; a `match`
  dispatches to handlers.

**Next:** [Chapter 7 — The commands](07-commands.md).
