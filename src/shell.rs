//! # Shell (REPL + Command Dispatcher)
//!
//! This module implements the kernel's interactive command shell.
//! It acts as the second stage of keyboard input handling:
//!
//! Keyboard Hardware
//!        ↓
//! Interrupt Handler
//!        ↓
//! Raw Scancodes
//!        ↓
//! Shell
//!
//! The shell is responsible for:
//!
//! 1. Converting raw keyboard scancodes into meaningful key events.
//! 2. Maintaining a command-line buffer while the user types.
//! 3. Handling simple line editing (Backspace).
//! 4. Detecting when a command is complete (Enter key).
//! 5. Parsing and executing built-in commands.
//!
//! Since the kernel runs without a heap allocator, all storage is
//! statically sized. The command line is stored in a fixed-size array
//! and command parsing operates directly on string slices (`&str`).
//! No heap allocations (`String`, `Vec`, etc.) are required.

use bootloader::BootInfo;
use bootloader::bootinfo::MemoryMap;
use pc_keyboard::{DecodedKey, HandleControl, KeyCode::C, Keyboard, ScancodeSet1, layouts};

use crate::{interrupts, print, println, vga_buffer};

/// Maximum number of characters allowed in a single command line.
///
/// The shell uses a fixed-size buffer because dynamic memory allocation
/// is not available. Once this limit is reached, additional characters
/// are ignored until the user presses Enter or removes characters.
const MAX_LINE: usize = 128;

/// Represents the entire runtime state of the shell.
///
/// The shell must remember:
/// - The keyboard decoder state (some keys produce multiple scancodes).
/// - The characters currently being typed.
/// - How much of the buffer is currently used.
/// - The bootloader's memory map for commands that inspect memory.
pub struct Shell {
    /// Converts incoming scancodes into decoded keyboard events
    /// using a US 104-key keyboard layout.
    keyboard: Keyboard<layouts::Us104Key, ScancodeSet1>,

    /// Fixed-size buffer holding the command currently being typed.
    /// Example: User types: "help"
    /// line = ['h', 'e', 'l', 'p', ...]
    /// len  = 4
    line: [u8; MAX_LINE],

    /// Number of valid bytes currently stored in `line`.
    /// This avoids scanning the entire buffer to determine where
    /// the command ends.
    len: usize,

    /// Memory map provided by the bootloader.
    /// The memory map has `'static` lifetime because it remains valid
    /// for the entire execution of the kernel.
    /// Commands such as `mem` can use it to display available and
    /// reserved memory regions.
    memory_map: &'static MemoryMap,
}

impl Shell {
    /// Creates a new shell instance.
    ///
    /// The shell starts with:
    /// - An empty command buffer.
    /// - Length set to zero.
    /// - A keyboard decoder configured for:
    ///   - Scancode Set 1 (standard PC keyboard scancodes).
    ///   - US keyboard layout.
    ///   - Control-key combinations treated as normal keys.
    pub fn new(memory_map: &'static MemoryMap) -> Self {
        Shell {
            keyboard: Keyboard::new(
                ScancodeSet1::new(),
                layouts::Us104Key,
                HandleControl::Ignore // Ignore special handling for Ctrl combinations. Ctrl+C will simply be decoded as a key  instead of triggering terminal-like behavior.
            ),
            line: [0; MAX_LINE],      // Initialize the command buffer with zeros.
            len: 0,                   // No characters have been entered yet.
            memory_map,
        }
    }

    /// Displays the startup banner and prints the first prompt.
    ///
    /// This is typically called once after the kernel finishes
    /// its initialization sequence.
    pub fn start(&self) {
        println!("=============================================");
        println!("  mini kernel shell  -  type 'help' to begin");
        println!("=============================================");

        self.prompt();
    }

    /// Prints the shell prompt shown before every command.
    /// kernel>
    fn prompt(&self) {
        print!("kernel> ");
    }

    /// Processes a single raw keyboard scancode.
    ///
    /// The keyboard interrupt handler forwards every received
    /// scancode to this function.
    ///
    /// Example:
    /// Pressing the 'A' key does not immediately produce the
    /// character 'a'. Instead:
    ///
    /// Hardware → Scancode → Decoder → Key Event → Character
    ///
    /// The keyboard decoder internally tracks multi-byte sequences
    /// and only emits an event once a complete key action has been
    /// recognized.
    pub fn feed_scancode(&mut self, scancode: u8) {
        // Feed the raw scancode into the decoder.
        // Some keys require multiple bytes before a complete
        // key event can be generated.
        if let Ok(Some(key_event)) = self.keyboard.add_byte(scancode) {

            // Convert the key event into a higher-level key representation.
            if let Some(key) = self.keyboard.process_keyevent(key_event) {
                match key {
                    // Regular printable character.
                    DecodedKey::Unicode(character) => {
                        self.on_char(character);
                    }

                    // Special keys such as arrows, function keys,
                    // Insert, Delete, etc.
                    //
                    // These are currently ignored because the shell
                    // only supports basic text input.
                    DecodedKey::RawKey(_) => {}
                }
            }
        }
    }

    /// Handles a decoded character produced by the keyboard decoder.
    ///
    /// Special characters trigger editing or command execution,
    /// while normal printable characters are appended to the buffer.
    fn on_char(&mut self, character: char) {
        match character {
            // User pressed Enter.
            // Execute the command currently stored in the buffer.
            '\n' => self.on_enter(),

            // User pressed Backspace.
            // Remove the most recently typed character.
            '\u{8}' => self.on_backspace(),

            // Normal printable ASCII character.
            //
            // Restricting input to printable ASCII keeps parsing
            // simple and avoids dealing with UTF-8 encoding in the
            // command buffer.
            c if c.is_ascii() && !c.is_ascii_control() => {
                self.push_char(c);
            }

            // Ignore unsupported characters.
            //
            // This includes:
            // - Control characters
            // - Unicode characters outside ASCII
            // - Any other special input
            _ => {}
        }
    }

    /// Appends a character to the command buffer and echoes it
    /// to the screen so the user can see what was typed.
    ///
    /// Example:
    ///
    /// Before:
    /// line = ['h', 'e']
    /// len  = 2
    ///
    /// User types 'l'
    ///
    /// After:
    /// line = ['h', 'e', 'l']
    /// len  = 3
    fn push_char(&mut self, character: char) {
        if self.len < MAX_LINE {
            // Store the character in the next free slot.
            self.line[self.len] = character as u8;

            // Advance the logical end of the command.
            self.len += 1;

            // Echo the character to the display.
            print!("{}", character);
        }

        // If the buffer is already full, the character is discarded.
        // This prevents writing past the end of the fixed-size array
        // and keeps the shell memory-safe.
    }

    /// Removes the most recently typed character from the current command.
    ///
    /// This function updates both:
    /// - The logical command buffer (`len`)
    /// - The visible text on the screen
    ///
    /// Example:
    /// User typed: "help"
    ///
    /// Buffer:
    /// ['h', 'e', 'l', 'p']
    /// len = 4
    ///
    /// After Backspace:
    /// ['h', 'e', 'l', 'p']
    /// len = 3
    ///
    /// Notice that the byte `'p'` remains in memory, but because `len`
    /// was reduced, it is no longer considered part of the command.
    /// Future typing will overwrite that position.
    fn on_backspace(&mut self) {
        if self.len > 0 {
            // Move the logical end of the command one character back.
            self.len -= 1;

            // Remove the character from the VGA display so the screen
            // stays synchronized with the command buffer.
            vga_buffer::backspace();
        }
    }

    /// Handles the Enter key.
    ///
    /// When Enter is pressed, the current contents of the line buffer
    /// become a complete command. The shell:
    ///
    /// 1. Moves to the next screen line.
    /// 2. Converts the typed bytes into a string.
    /// 3. Executes the command.
    /// 4. Clears the current buffer state.
    /// 5. Prints a fresh prompt for the next command.
    fn on_enter(&mut self) {
        // Move the cursor to the next line so command output appears
        // beneath the typed command.
        println!();

        // Convert the used portion of the byte buffer into a string.
        //
        // Only bytes from 0..len are part of the command; the rest of
        // the array is unused space.
        //
        // Because keyboard input is restricted to ASCII characters,
        // the buffer should always contain valid UTF-8 data.
        // `unwrap_or("")` is simply a defensive fallback.
        let input = core::str::from_utf8(&self.line[..self.len])
            .unwrap_or("")
            .trim();

        // Parse and execute the command.
        self.dispatch(input);

        // Reset the command buffer for the next line.
        //
        // We only reset `len`; the old bytes remain in memory but are
        // ignored because the buffer is considered empty.
        self.len = 0;

        // Display the next prompt.
        self.prompt();
    }

    /// Parses a completed command line and dispatches it to the
    /// appropriate built-in command handler.
    ///
    /// Example inputs:
    ///
    /// "help"
    ///     command = "help"
    ///     args    = ""
    ///
    /// "echo hello world"
    ///     command = "echo"
    ///     args    = "hello world"
    ///
    /// "sleep 1000"
    ///     command = "sleep"
    ///     args    = "1000"
    ///
    /// The command name determines which handler function is called.
    fn dispatch(&self, input: &str) {
        // Ignore blank lines so pressing Enter repeatedly does not
        // generate errors or unnecessary output.
        if input.is_empty() {
            return;
        }

        // Split the input into:
        //   command + remaining arguments
        //
        // splitn(2, ...) stops after the first split, preserving the
        // remainder exactly as typed.
        //
        // Example:
        //   "echo hello world"
        //
        // becomes:
        //   command = "echo"
        //   args    = "hello world"
        //
        // This is important for commands like `echo` where whitespace
        // inside the argument string should be preserved.
        let mut parts = input.splitn(2, char::is_whitespace);

        // First token is the command name.
        let command = parts.next().unwrap_or("");

        // Everything after the first whitespace becomes the argument string.
        let args = parts.next().unwrap_or("").trim_start();

        // Dispatch to the matching command implementation.
        //
        // Each command corresponds to a dedicated handler method.
        match command {
            "help" => self.cmd_help(),

            // Unknown command fallback.
            other => {
                println!(
                    "unknown command: '{}'  (type 'help')",
                    other
                );
            }
        }
    }

    /// `help` — list the available commands.
    fn cmd_help(&self) {
        println!("available commands:");
        println!("  help            show this help text");
    }
}