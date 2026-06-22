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

use bootloader::bootinfo::MemoryMap;
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};

use crate::{interrupts, print, println, vga_buffer};

/// Maximum number of characters allowed in a single command line.
///
/// The shell uses a fixed-size buffer because dynamic memory allocation
/// is not available. Once this limit is reached, additional characters
/// are ignored until the user presses Enter or removes characters.
const MAX_LINE: usize = 128;

/// Return "yes"/"no" for whether bit `n` is set in `value`. Used to print
/// CPU feature flags readably in the `cpuid` command.
fn bit(value: u32, n: u32) -> &'static str {
    if value & (1 << n) != 0 {
        "yes"
    } else {
        "no"
    }
}

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
                HandleControl::Ignore, // Ignore special handling for Ctrl combinations. Ctrl+C will simply be decoded as a key  instead of triggering terminal-like behavior.
            ),
            line: [0; MAX_LINE], // Initialize the command buffer with zeros.
            len: 0,              // No characters have been entered yet.
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
            "help" => self.cmd_help(),             // Show available shell commands and usage help
            "clear" => vga_buffer::clear_screen(), // Clear the VGA text buffer and reset display state
            "echo" => println!("{}", args),          // Print the provided argument string to the screen
            "reboot" => self.cmd_reboot(),         // Attempt to reboot the system
            "shutdown" => self.cmd_shutdown(),     // Attempt to shut down the system
            "mem" => self.cmd_mem(),               // Display memory map information from the bootloader
            "cpuid" => self.cmd_cpuid(),           // Display CPU identification (vendor, features, etc.)
            "tsc" => self.cmd_tsc(),               // Read and display CPU Time Stamp Counter value
            "uptime" => self.cmd_uptime(),         // Show elapsed time since kernel boot
            "sleep" => self.cmd_sleep(args),       // Busy-wait or delay execution for given duration
            "time" => self.cmd_time(),             // Display current system/kernel time information
            "colors" => self.cmd_colors(),         // Demonstrate VGA color palette output
            "peek" => self.cmd_peek(args),         // Read memory from a specified address
            "int3" => self.cmd_int3(),             // Trigger software breakpoint interrupt (INT3)
            "panic" => self.cmd_panic(args),       // Deliberately trigger a kernel panic
            other => {
                println!("unknown command: '{}'  (type 'help')", other); // Handle unknown commands with a fallback message
            }
        }
    }

    /// `help` — list the available commands.
    fn cmd_help(&self) {
        println!("available commands:");
        println!("  help            show this help text");
        println!("  clear           clear the screen");
        println!("  echo <text>     print the given text back");
        println!("  reboot          reset the machine");
        println!("  shutdown        power off (QEMU/ACPI)");
        println!("  mem             show the real physical memory map");
        println!("  cpuid           show CPU vendor and features");
        println!("  tsc             read the CPU timestamp counter (cycles)");
        println!("  uptime          time since boot (from the timer interrupt)");
        println!("  sleep <ticks>   wait n timer ticks (~18 ticks = 1 second)");
        println!("  time            wall-clock time from the RTC (CMOS clock)");
        println!("  colors          show the 16 VGA text colors");
        println!("  peek <hex> [n]  read n bytes from a memory address");
        println!("  int3            fire a breakpoint exception and recover");
        println!("  panic [msg]     trigger a kernel panic (halts the CPU)");
    }

    // -----------------------------------------------------------------------------
    // Available functions
    // -----------------------------------------------------------------------------

    /// `panic` — deliberately trigger a kernel panic.
    ///
    /// In a bare-metal environment (no OS), `panic!` does not unwind to an OS
    /// handler like in user-space programs. Instead, it invokes the kernel’s
    /// panic handler, which typically prints diagnostic information (often in
    /// a highlighted color) and halts execution permanently.
    ///
    /// Since there is no higher-level system to recover to, execution cannot
    /// continue after this point.
    fn cmd_panic(&self, args: &str) -> ! {
        if args.is_empty() {
            panic!("manual panic triggered from the shell");
        } else {
            panic!("{}", args);
        }
    }

    /// `reboot` — perform a CPU reset via the legacy 8042 keyboard controller.
    ///
    /// On classic x86 hardware, the 8042 controller (commonly used for keyboard
    /// input) also exposes a reset command. Writing `0xFE` to I/O port `0x64`
    /// triggers a hardware reset pulse, effectively rebooting the machine.
    ///
    /// This method is widely used in bare-metal OS development because it works
    /// without needing ACPI or chipset-specific reset registers.
    fn cmd_reboot(&self) -> ! {
        use x86_64::instructions::port::Port;

        println!("rebooting...");

        // The command port of the 8042 controller
        let mut port: Port<u8> = Port::new(0x64);

        unsafe {
            // 0xFE = "Pulse CPU reset line"
            port.write(0xFEu8);
        }

        // If the reset does not occur (e.g., emulator quirks), we must not
        // continue executing arbitrary code. Instead, halt the CPU safely.
        loop {
            x86_64::instructions::hlt();
        }
    }

    /// `shutdown` — attempt to power off the system using ACPI S5 ("soft off").
    ///
    /// Unlike rebooting, powering off a machine is not standardized at the
    /// simple I/O level on x86. The modern mechanism is ACPI (Advanced
    /// Configuration and Power Interface), which defines system states such as:
    /// - S0: working state
    /// - S5: soft-off (system is effectively powered down)
    ///
    /// Different firmware/VM implementations expose different I/O ports for
    /// triggering S5. Here we attempt multiple commonly used ports so that at
    /// least one works across QEMU and some legacy firmware setups.
    ///
    /// In a production kernel, the correct approach is to parse ACPI tables
    /// (FADT/PM1a control blocks) and use the platform-defined shutdown port.
    fn cmd_shutdown(&self) -> ! {
        use x86_64::instructions::port::Port;

        println!("shutting down...");

        // Try multiple known ACPI power management ports (varies by system/VM)
        for port_addr in [0x604u16, 0xB004, 0x4004] {
            let mut port: Port<u16> = Port::new(port_addr);
            unsafe {
                // 0x2000 is commonly associated with entering ACPI S5 state
                port.write(0x2000u16);
            }
        }

        // QEMU-specific fallback:
        // The `isa-debug-exit` device allows the VM to terminate when written to.
        let mut debug_exit: Port<u32> = Port::new(0xf4);
        unsafe {
            debug_exit.write(0x10u32);
        }

        // If none of the shutdown methods worked (e.g., real hardware without
        // matching ACPI configuration), ensure we do not continue execution.
        loop {
            x86_64::instructions::hlt();
        }
    }

    /// `mem` — display the physical memory map provided by the bootloader.
    ///
    /// This memory map comes directly from the bootloader’s firmware queries
    /// (e.g., E820 on BIOS systems or UEFI memory descriptors).
    /// Each entry describes a physical address range and its type:
    /// - Usable RAM (safe for allocation)
    /// - Reserved regions (firmware / hardware)
    /// - Kernel / bootloader memory
    /// - ACPI tables, etc.
    ///
    /// This is the authoritative view of physical memory layout.
    fn cmd_mem(&self) {
        println!("physical memory map (from bootloader):");

        let mut usable: u64 = 0;

        for region in self.memory_map.iter() {
            let start = region.range.start_addr();
            let end = region.range.end_addr();

            println!(
                "  {:#012x} - {:#012x}  {:?}",
                start, end, region.region_type
            );

            // Only "Usable" regions can be used by a physical memory allocator.
            if region.region_type == bootloader::bootinfo::MemoryRegionType::Usable {
                usable += end - start;
            }
        }

        // Convert to KiB for human-readable output.
        println!("  usable RAM: {} KiB", usable / 1024);
    }

    /// `cpuid` — query CPU capabilities using the CPUID instruction.
    ///
    /// CPUID is the standard x86 mechanism for discovering CPU information.
    /// Input:
    ///   - EAX = "leaf" (function selector)
    /// Output:
    ///   - EAX, EBX, ECX, EDX contain feature/vendor data depending on leaf.
    ///
    /// Leaf 0 returns:
    ///   - EBX + EDX + ECX = 12-byte CPU vendor string
    ///   - EAX = highest supported CPUID leaf
    ///
    /// Leaf 1 returns:
    ///   - Feature flags in EDX (legacy feature bitmap)
    fn cmd_cpuid(&self) {
        use core::arch::x86_64::__cpuid;

        // Leaf 0: vendor ID + max supported leaf
        let leaf0 = __cpuid(0);

        // Vendor string is stored as three 32-bit registers:
        // EBX, EDX, ECX (in that order).
        let mut vendor = [0u8; 12];

        vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

        let vendor = core::str::from_utf8(&vendor).unwrap_or("?");

        println!("CPU vendor: {}", vendor);
        println!("max cpuid leaf: {}", leaf0.eax);

        // Leaf 1: legacy feature flags (EDX bitfield)
        let leaf1 = __cpuid(1);
        let edx = leaf1.edx;

        println!("features:");
        println!("  FPU   (x87 float)  : {}", bit(edx, 0));
        println!("  TSC   (timestamp)  : {}", bit(edx, 4));
        println!("  APIC  (local APIC) : {}", bit(edx, 9));
        println!("  SSE                : {}", bit(edx, 25));
        println!("  SSE2               : {}", bit(edx, 26));
    }

    /// `tsc` — read the Time Stamp Counter (TSC).
    ///
    /// The TSC is a 64-bit CPU register that increments at (roughly) every CPU
    /// cycle. It is the highest-resolution timing source available directly from
    /// the processor.
    ///
    /// It is useful for:
    /// - benchmarking code
    /// - measuring short time intervals
    /// - profiling low-level kernel paths
    fn cmd_tsc(&self) {
        // rdtsc reads the CPU's timestamp counter.
        let cycles = unsafe { core::arch::x86_64::_rdtsc() };

        println!("timestamp counter: {} cycles since CPU reset", cycles);
        println!("(run again — value always increases)");
    }

    /// `uptime` — system uptime derived from the PIT timer interrupt.
    ///
    /// This uses the periodic timer interrupt (≈18.2 Hz in legacy PIT mode),
    /// which increments a global tick counter.
    ///
    /// Compared to TSC:
    /// - TSC: high precision, CPU-cycle based
    /// - ticks: low precision, interrupt-based
    fn cmd_uptime(&self) {
        let ticks = interrupts::ticks();

        // Convert ticks → tenths of seconds using integer arithmetic.
        let tenths = ticks * 10 / interrupts::TIMER_HZ_X10;

        println!(
            "uptime: {} ticks  (~{}.{} seconds at ~18.2 Hz)",
            ticks,
            tenths / 10,
            tenths % 10
        );
    }

    /// `sleep <ticks>` — busy-wait using `hlt` until timer ticks advance.
    ///
    /// There is no scheduler yet, so “sleeping” means:
    /// - wait for timer interrupts
    /// - halt CPU between interrupts to avoid busy spinning
    fn cmd_sleep(&self, args: &str) {
        let n: u64 = match args.trim().parse() {
            Ok(value) => value,
            Err(_) => {
                println!("usage: sleep <ticks> (~18 ticks = 1 second)");
                return;
            }
        };

        // Prevent accidental long stalls from huge inputs.
        let n = n.min(100_000);

        let start = interrupts::ticks();
        let target = start + n;

        println!("sleeping {} ticks...", n);

        while interrupts::ticks() < target {
            // HLT pauses CPU until next interrupt (power-efficient idle loop)
            x86_64::instructions::hlt();
        }

        println!("awake after {} ticks", interrupts::ticks() - start);
    }

    /// `time` — read current time from the CMOS Real-Time Clock (RTC).
    ///
    /// The RTC is a battery-backed hardware clock present in legacy PC systems.
    /// It is accessed through I/O ports:
    /// - 0x70: register selector
    /// - 0x71: data port
    ///
    /// Many RTC values are stored in BCD (Binary-Coded Decimal).
    fn cmd_time(&self) {
        use x86_64::instructions::port::Port;

        // Read a CMOS register via ports 0x70/0x71.
        fn read_cmos(reg: u8) -> u8 {
            let mut addr: Port<u8> = Port::new(0x70);
            let mut data: Port<u8> = Port::new(0x71);

            unsafe {
                addr.write(reg);
                data.read()
            }
        }

        // Wait until RTC is not updating (prevents inconsistent reads).
        while read_cmos(0x0A) & 0x80 != 0 {}

        let second = read_cmos(0x00);
        let minute = read_cmos(0x02);
        let hour_raw = read_cmos(0x04);
        let day = read_cmos(0x07);
        let month = read_cmos(0x08);
        let year = read_cmos(0x09);
        let status_b = read_cmos(0x0B);

        // Decode RTC format flags:
        let is_bcd = status_b & 0x04 == 0;
        let is_24h = status_b & 0x02 != 0;

        // Convert BCD → binary if needed.
        let from_bcd = |v: u8| {
            if is_bcd {
                (v >> 4) * 10 + (v & 0x0f)
            } else {
                v
            }
        };

        // Handle 12-hour mode (PM bit stored in MSB of hour register).
        let pm = !is_24h && (hour_raw & 0x80 != 0);
        let mut hour = from_bcd(hour_raw & 0x7f);

        if pm && hour != 12 {
            hour += 12;
        }

        println!(
            "RTC time (UTC): 20{:02}-{:02}-{:02} {:02}:{:02}:{:02}",
            from_bcd(year),
            from_bcd(month),
            from_bcd(day),
            hour,
            from_bcd(minute),
            from_bcd(second),
        );
    }

    /// `colors` — display VGA text-mode 16-color palette.
    ///
    /// VGA text mode uses a 4-bit color index:
    /// - 0–15 represent fixed hardware-defined colors
    /// - each entry controls foreground/background text colors
    fn cmd_colors(&self) {
        use vga_buffer::Color::*;

        let palette = [
            (Black, "Black"),
            (Blue, "Blue"),
            (Green, "Green"),
            (Cyan, "Cyan"),
            (Red, "Red"),
            (Magenta, "Magenta"),
            (Brown, "Brown"),
            (LightGray, "LightGray"),
            (DarkGray, "DarkGray"),
            (LightBlue, "LightBlue"),
            (LightGreen, "LightGreen"),
            (LightCyan, "LightCyan"),
            (LightRed, "LightRed"),
            (Pink, "Pink"),
            (Yellow, "Yellow"),
            (White, "White"),
        ];

        println!("VGA text-mode palette (16 colors, 4-bit):");

        for (color, name) in palette {
            print!("  ");

            // Color swatch using background color
            vga_buffer::print_colored("    ", vga_buffer::Color::Black, color);

            print!(" ");

            // Label rendered in its own color
            vga_buffer::print_colored(name, color, vga_buffer::Color::Black);

            println!("  (code {})", color as u8);
        }
    }

    /// `int3` — execute the one-byte `int3` instruction, which raises a
    /// breakpoint exception. The key lesson: our breakpoint handler prints and
    /// RETURNS, so execution continues normally afterwards. Contrast with
    /// `panic`, which never returns. This is how exceptions differ from crashes.
    fn cmd_int3(&self) {
        println!("triggering a breakpoint exception (int3)...");
        x86_64::instructions::interrupts::int3();
        println!("...and we're back! the exception was handled and recovered.");
    }

    /// `peek <hex> [n]` — read `n` bytes (default 16) starting at a memory
    /// address and print them as a hex dump. Demonstrates raw memory access.
    /// Reading an unmapped address triggers our page-fault handler.
    fn cmd_peek(&self, args: &str) {
        let mut parts = args.split_whitespace();

        // Parse the address (accepts an optional "0x" prefix).
        let addr_str = parts.next().unwrap_or("");
        let addr_str = addr_str.strip_prefix("0x").unwrap_or(addr_str);
        let addr = match u64::from_str_radix(addr_str, 16) {
            Ok(value) => value,
            Err(_) => {
                println!("usage: peek <hex-address> [count]   e.g. peek 0xb8000 32");
                return;
            }
        };

        // Parse the optional byte count (decimal), default 16, capped at 256.
        let count: u64 = parts
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(16)
            .min(256);

        println!("memory at {:#x}:", addr);
        for row in 0..count.div_ceil(16) {
            let base = addr + row * 16;
            print!("  {:#012x}: ", base);
            for col in 0..16 {
                if row * 16 + col >= count {
                    break;
                }
                // SAFETY: a bad address faults into our page-fault handler
                // rather than silently corrupting anything.
                let byte = unsafe { core::ptr::read_volatile((base + col) as *const u8) };
                print!("{:02x} ", byte);
            }
            println!();
        }
    }
}
