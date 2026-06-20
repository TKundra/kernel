//! # Mini kernel shell
//!
//! A small x86-64 operating system kernel written in Rust.
//!
//! Unlike a normal program, this runs directly on the hardware with no
//! operating system underneath. After booting, it initializes the basic
//! systems needed to provide an interactive command shell.
//!
//! Main components:
//!
//! * `vga_buffer`  — Displays text on the screen.
//! * `gdt`         — Sets up CPU structures needed for fault handling.
//! * `interrupts`  — Handles hardware interrupts such as keyboard input.
//! * `shell`       — Reads commands and executes built-in functionality.
//!
//! Boot sequence:
//!
//! BIOS → bootloader → `kernel_main()` → shell

// A kernel cannot use Rust's standard library because `std` requires an
// operating system. We also provide our own entry point instead of `main()`.
#![no_std]
#![no_main]

// Interrupt handlers require the special `x86-interrupt` calling convention,
// which currently requires a nightly Rust feature.
// #![feature(abi_x86_interrupt)]

pub mod vga_buffer;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};

// Generates the low-level entry point (`_start`) and ensures that
// `kernel_main` has the correct signature expected by the bootloader.
entry_point!(kernel_main);

/// First Rust function executed after the bootloader hands control to the
/// kernel.
///
/// `boot_info` contains information gathered during boot, including the
/// physical memory map. The `!` return type means this function never returns,
/// since there is no operating system to return to.
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    println!("Hello VGA world!");
    println!("This is a bare-metal kernel.");
    println!("Numbers: {} {}", 42, 1337);

    loop {}
}

/// Called whenever the kernel encounters an unrecoverable error.
///
/// Since there is no higher-level system to handle the failure, the kernel
/// prints diagnostic information and stops execution permanently.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("PANIC: {}", info);
    
    // Halt the CPU forever. The `hlt` instruction puts the processor into a
    // low-power idle state until an interrupt occurs, while the surrounding
    // loop prevents execution from continuing.
    loop {
        x86_64::instructions::hlt();
    }
}