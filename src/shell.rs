//! # The shell (REPL + commands)
//!
//! This is the "bottom half" of keyboard handling and the actual command
//! cockpit. The main loop feeds us raw scancodes one at a time; we:
//!
//!   1. Decode each scancode into a key (via the `pc-keyboard` crate).
//!   2. Build up a line in a fixed-size buffer, echoing characters to screen
//!      and handling Backspace/Enter (the "line discipline").
//!   3. On Enter, parse the line and run the matching built-in command.
//!
//! There is no heap, so the line buffer is a fixed `[u8; N]` array and all
//! parsing works on string slices — no `String`/`Vec` anywhere.

use bootloader::bootinfo::MemoryMap;
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};

use crate::{interrupts, print, println, vga_buffer};

/// Maximum characters in one command line. Anything beyond this is ignored.
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

/// The shell's state: the keyboard decoder, the current line buffer, and a
/// reference to the real memory map (so `mem` can print it).
pub struct Shell {
    keyboard: Keyboard<layouts::Us104Key, ScancodeSet1>,
    line: [u8; MAX_LINE],
    len: usize,
    memory_map: &'static MemoryMap,
}

impl Shell {
    /// Create a new shell. The memory map comes from the bootloader and lives
    /// for the whole run of the kernel (`'static`).
    pub fn new(memory_map: &'static MemoryMap) -> Self {
        Shell {
            // ScancodeSet1 + a US layout. `HandleControl::Ignore` means Ctrl+key
            // combos are passed through as ordinary keys rather than mapped to
            // control codes.
            keyboard: Keyboard::new(
                ScancodeSet1::new(),
                layouts::Us104Key,
                HandleControl::Ignore,
            ),
            line: [0; MAX_LINE],
            len: 0,
            memory_map,
        }
    }

    /// Print the boot banner and the first prompt.
    pub fn start(&self) {
        println!("=============================================");
        println!("  mini kernel shell  -  type 'help' to begin");
        println!("=============================================");
        self.prompt();
    }

    /// Print the command prompt.
    fn prompt(&self) {
        print!("kernel> ");
    }

    /// Feed one raw scancode into the shell. Decodes it and, if it produced a
    /// character, handles it. Called by the main loop for every scancode.
    pub fn feed_scancode(&mut self, scancode: u8) {
        // `add_byte` assembles multi-byte scancodes; it returns a key event
        // only once a full key press/release is decoded.
        if let Ok(Some(key_event)) = self.keyboard.add_byte(scancode) {
            if let Some(key) = self.keyboard.process_keyevent(key_event) {
                match key {
                    // A normal character key.
                    DecodedKey::Unicode(character) => self.on_char(character),
                    // Arrow keys, F-keys, etc. Ignored for now.
                    DecodedKey::RawKey(_) => {}
                }
            }
        }
    }

    /// Handle one decoded character, dispatching on the special ones.
    fn on_char(&mut self, character: char) {
        match character {
            '\n' => self.on_enter(),     // Enter: run the line
            '\u{8}' => self.on_backspace(), // Backspace (ASCII 0x08)
            // Any other printable ASCII character goes into the buffer.
            c if c.is_ascii() && !c.is_ascii_control() => self.push_char(c),
            // Ignore everything else (control chars, non-ASCII).
            _ => {}
        }
    }

    /// Append a character to the line buffer and echo it to the screen.
    fn push_char(&mut self, character: char) {
        if self.len < MAX_LINE {
            self.line[self.len] = character as u8;
            self.len += 1;
            print!("{}", character);
        }
        // If the line is full we silently drop the character.
    }

    /// Remove the last character from the buffer and erase it on screen.
    fn on_backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            vga_buffer::backspace();
        }
    }

    /// Enter pressed: terminate the line, run it, reset the buffer, re-prompt.
    fn on_enter(&mut self) {
        println!(); // move to the next line on screen

        // Interpret the raw bytes as a string slice. Keyboard input is ASCII,
        // so this is always valid UTF-8, but we fall back to empty just in case.
        let input = core::str::from_utf8(&self.line[..self.len])
            .unwrap_or("")
            .trim();

        self.dispatch(input);

        // Reset for the next command and show a fresh prompt.
        self.len = 0;
        self.prompt();
    }

    /// Parse a finished line and run the matching command.
    fn dispatch(&self, input: &str) {
        if input.is_empty() {
            return; // empty line: do nothing
        }

        // Split into the command word and "the rest" (its arguments). We split
        // on the FIRST run of whitespace so `echo` can keep its spacing.
        let mut parts = input.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("").trim_start();

        match command {
            "help" => self.cmd_help(),
            "clear" => vga_buffer::clear_screen(),
            "echo" => println!("{}", args),
            "mem" => self.cmd_mem(),
            "regs" => self.cmd_regs(),
            "cpuid" => self.cmd_cpuid(),
            "uptime" => self.cmd_uptime(),
            "int3" => self.cmd_int3(),
            "peek" => self.cmd_peek(args),
            "reboot" => self.cmd_reboot(),
            "shutdown" => self.cmd_shutdown(),
            "panic" => self.cmd_panic(args),
            other => println!("unknown command: '{}'  (type 'help')", other),
        }
    }

    /// `help` — list the available commands.
    fn cmd_help(&self) {
        println!("available commands:");
        println!("  help            show this help text");
        println!("  clear           clear the screen");
        println!("  echo <text>     print the given text back");
        println!("  mem             show the real physical memory map");
        println!("  regs            dump CPU control registers + flags");
        println!("  cpuid           show CPU vendor and features");
        println!("  uptime          time since boot (from the timer interrupt)");
        println!("  int3            fire a breakpoint exception and recover");
        println!("  peek <hex> [n]  read n bytes from a memory address");
        println!("  reboot          reset the machine");
        println!("  shutdown        power off (QEMU/ACPI)");
        println!("  panic [msg]     trigger a kernel panic (halts the CPU)");
    }

    /// `mem` — print the REAL memory map the bootloader discovered. This is the
    /// genuine article now: each region is a physical address range and a type
    /// (usable RAM, reserved, bootloader/kernel, etc.).
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

            // Tally up usable RAM so we can print a total.
            if region.region_type == bootloader::bootinfo::MemoryRegionType::Usable {
                usable += end - start;
            }
        }

        // Convert bytes to KiB for a readable total.
        println!("  usable RAM: {} KiB", usable / 1024);
    }

    /// `regs` — dump the x86-64 control registers and flags.
    ///
    /// Control registers steer how the CPU behaves at the lowest level:
    ///   CR0 — mode bits (protected mode, paging on/off, etc.)
    ///   CR2 — the address of the last page fault
    ///   CR3 — physical address of the top-level page table (the "page map")
    ///   CR4 — extension bits (enables features like the timestamp counter)
    ///   RFLAGS — status/condition flags, including whether interrupts are on
    fn cmd_regs(&self) {
        use x86_64::registers::control::{Cr0, Cr2, Cr3, Cr4};
        use x86_64::registers::rflags::{self, RFlags};

        let cr3 = Cr3::read();
        println!("CPU registers:");
        println!("  CR0    = {:#018x}", Cr0::read().bits());
        // CR2 holds the last faulting address; usually 0 if no fault yet.
        println!("  CR2    = {:#018x}", Cr2::read().as_u64());
        println!(
            "  CR3    = {:#018x}  (page table root)",
            cr3.0.start_address().as_u64()
        );
        println!("  CR4    = {:#018x}", Cr4::read().bits());
        let flags = rflags::read();
        println!("  RFLAGS = {:#018x}", flags.bits());
        // Bit 9 of RFLAGS is the Interrupt Flag: are hardware interrupts on?
        println!(
            "    interrupts: {}",
            if flags.contains(RFlags::INTERRUPT_FLAG) {
                "enabled"
            } else {
                "disabled"
            }
        );
    }

    /// `cpuid` — ask the CPU to identify itself via the `cpuid` instruction.
    ///
    /// `cpuid` takes a "leaf" number in EAX and returns data in EAX/EBX/ECX/EDX.
    /// Leaf 0 returns the 12-character vendor string (e.g. "GenuineIntel" or, on
    /// QEMU, "AuthenticAMD"/"TCGTCGTCGTCG"). Leaf 1 returns feature flags.
    fn cmd_cpuid(&self) {
        use core::arch::x86_64::__cpuid;

        // cpuid has no side effects; it just reads CPU capabilities.
        let leaf0 = __cpuid(0);

        // The vendor string is the bytes of EBX, then EDX, then ECX.
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());
        let vendor = core::str::from_utf8(&vendor).unwrap_or("?");

        println!("CPU vendor: {}", vendor);
        println!("max cpuid leaf: {}", leaf0.eax);

        // Leaf 1: a handful of well-known feature bits in EDX.
        let leaf1 = __cpuid(1);
        let edx = leaf1.edx;
        println!("features:");
        println!("  FPU   (x87 float)  : {}", bit(edx, 0));
        println!("  TSC   (timestamp)  : {}", bit(edx, 4));
        println!("  APIC  (local APIC) : {}", bit(edx, 9));
        println!("  SSE                : {}", bit(edx, 25));
        println!("  SSE2               : {}", bit(edx, 26));
    }

    /// `uptime` — how long since boot, derived from the timer interrupt's tick
    /// counter. This is the timer interrupt finally being useful.
    fn cmd_uptime(&self) {
        let ticks = interrupts::ticks();
        // seconds = ticks / 18.2; we kept the frequency x10 to avoid floats.
        let tenths = ticks * 10 / interrupts::TIMER_HZ_X10;
        println!(
            "uptime: {} ticks  (~{}.{} seconds at ~18.2 Hz)",
            ticks,
            tenths / 10,
            tenths % 10
        );
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

    /// `reboot` — reset the machine by pulsing the CPU-reset line through the
    /// legacy 8042 keyboard controller (port 0x64). A classic PC trick.
    fn cmd_reboot(&self) -> ! {
        use x86_64::instructions::port::Port;

        println!("rebooting...");
        let mut port: Port<u8> = Port::new(0x64);
        unsafe { port.write(0xFEu8) }; // pulse the reset line

        // If the reset somehow didn't take, halt rather than run on.
        loop {
            x86_64::instructions::hlt();
        }
    }

    /// `shutdown` — power off. There's no single standard "off switch" on bare
    /// x86; power management goes through ACPI, and the exact I/O port differs
    /// between virtual machines and firmware generations. We write the ACPI S5
    /// ("soft off") command (0x2000) to each of the ports used by the common
    /// QEMU/firmware versions, so at least one takes effect. On real hardware
    /// you would parse the ACPI tables to find the right port.
    fn cmd_shutdown(&self) -> ! {
        use x86_64::instructions::port::Port;

        println!("shutting down...");
        for port_addr in [0x604u16, 0xB004, 0x4004] {
            let mut port: Port<u16> = Port::new(port_addr);
            unsafe { port.write(0x2000u16) };
        }

        // Fallback for QEMU: the `isa-debug-exit` device (wired up in
        // Cargo.toml's run-args) makes QEMU exit when we write to port 0xf4.
        let mut debug_exit: Port<u32> = Port::new(0xf4);
        unsafe { debug_exit.write(0x10u32) };

        // If nothing powered us off (e.g. real hardware without these), halt.
        loop {
            x86_64::instructions::hlt();
        }
    }

    /// `panic` — deliberately crash. Unlike the hosted version, this is a REAL
    /// kernel panic: the panic handler prints in red and halts the CPU. There
    /// is no OS to return to, so the machine simply stops.
    fn cmd_panic(&self, args: &str) -> ! {
        if args.is_empty() {
            panic!("manual panic triggered from the shell");
        } else {
            panic!("{}", args);
        }
    }
}
