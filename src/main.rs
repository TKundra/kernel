//! # Mini kernel shell — bare-metal x86-64 kernel
//!
//! This is a real freestanding kernel: it boots on bare hardware (or QEMU)
//! with no operating system underneath. It brings up just enough of the
//! machine to run an interactive command shell:
//!
//!   * `vga_buffer` — writes text to the screen at physical address 0xb8000.
//!   * `gdt`        — sets up a safety-net stack for fatal faults.
//!   * `interrupts` — installs the interrupt table and a keyboard driver.
//!   * `shell`      — the REPL: decodes keys, edits a line, runs commands.
//!
//! Boot flow:  BIOS -> bootloader -> `kernel_main` (below) -> shell loop.

// We have no standard library (it assumes an OS) and no normal `main` (the
// bootloader calls our entry point directly).
#![no_std]
#![no_main]
// The `x86-interrupt` calling convention is required for interrupt handlers and
// is still unstable, so we must opt in.
#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;

use bootloader::{entry_point, BootInfo};

// Bring our modules into the build. (`vga_buffer` is referenced by the
// `print!`/`println!` macros it exports, hence `#[macro_use]`-style usage via
// `$crate`.)
mod gdt;
mod interrupts;
mod shell;
mod vga_buffer;

// `entry_point!` generates the low-level `_start` symbol the bootloader jumps
// to, and type-checks our `kernel_main` signature for us.
entry_point!(kernel_main);

/// The kernel's entry point. `boot_info` is provided by the bootloader and
/// includes the physical memory map. The return type `!` means it never
/// returns — there is nothing to return to.
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // ---- Bring up the CPU's fault/interrupt machinery ----
    gdt::init(); // load GDT + TSS (double-fault safety stack)
    interrupts::init_idt(); // install the interrupt descriptor table
    unsafe { interrupts::PICS.lock().initialize() }; // wire up the PIC
    x86_64::instructions::interrupts::enable(); // let interrupts fire

    // ---- Start the shell ----
    let mut shell = shell::Shell::new(&boot_info.memory_map);
    shell.start();

    // ---- The REPL loop ----
    // Keys arrive asynchronously via the keyboard interrupt, which fills a
    // queue. Here we drain that queue and feed each scancode to the shell.
    loop {
        match interrupts::next_scancode() {
            // A key is waiting — process it.
            Some(scancode) => shell.feed_scancode(scancode),
            // Nothing to do. Atomically enable interrupts and halt the CPU so
            // it sleeps until the next interrupt, instead of spinning hot.
            None => x86_64::instructions::interrupts::enable_and_hlt(),
        }
    }
}

/// The panic handler — required for every `no_std` binary, and our real
/// "kernel panic" path. We can't recover (there's no OS), so we print the
/// reason in red and halt the CPU forever.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    vga_buffer::set_panic_color();
    println!("\n*** KERNEL PANIC ***");
    println!("{}", info);

    // Halt in a loop. `hlt` parks the CPU until an interrupt; the surrounding
    // loop means we never wake back into normal execution.
    loop {
        x86_64::instructions::hlt();
    }
}
