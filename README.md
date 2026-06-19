# Mini Kernel Shell (bare-metal x86-64)

A **real freestanding kernel** that boots on bare hardware (or QEMU) with no
operating system underneath, and drops you into an interactive command shell —
your **debugging cockpit**: `help`, `clear`, `echo`, `mem`, `panic`.

It is `#![no_std]` + `#![no_main]`, talks to the screen by writing directly to
VGA memory, reads the keyboard through a real hardware-interrupt driver, and —
unlike a hosted toy — the `mem` command prints the **actual physical memory
map** and `panic` is a **real kernel panic** that halts the CPU.

> Verified booting in QEMU: banner + prompt render, keystrokes are decoded by
> the interrupt-driven keyboard driver, and all commands work.

---

## Table of contents

- [Mini Kernel Shell (bare-metal x86-64)](#mini-kernel-shell-bare-metal-x86-64)
  - [Table of contents](#table-of-contents)
  - [What "bare metal" means here](#what-bare-metal-means-here)
  - [Boot flow](#boot-flow)
  - [Architecture](#architecture)
  - [Module map](#module-map)
  - [How a keypress becomes a command](#how-a-keypress-becomes-a-command)
  - [The interrupt path (top half / bottom half)](#the-interrupt-path-top-half--bottom-half)
  - [Memory: VGA + the real memory map](#memory-vga--the-real-memory-map)
  - [Commands](#commands)
  - [Build \& run](#build--run)
  - [The toolchain setup (and why each piece exists)](#the-toolchain-setup-and-why-each-piece-exists)
  - [Gotchas we hit (and the fixes)](#gotchas-we-hit-and-the-fixes)
  - [File layout](#file-layout)

---

## What "bare metal" means here

There is **no OS** beneath this code. That changes everything compared to a
normal Rust program:

| Normal Rust program            | This kernel                                  |
|--------------------------------|----------------------------------------------|
| `std` library                  | `#![no_std]` — only `core` is available      |
| `fn main()`                    | `#![no_main]` — the bootloader calls `_start`|
| `println!` via the OS          | write bytes straight into VGA memory         |
| OS reads the keyboard          | we handle the keyboard hardware interrupt    |
| heap / `Vec` / `String`        | none — fixed arrays only                     |
| panic unwinds                  | panic halts the CPU                          |
| OS gives you memory            | the bootloader hands us a physical memory map|

---

## Boot flow

```
   power on
      |
      v
   +--------+    +--------------------+    +-------------------------+
   | BIOS   | -> | bootloader (crate) | -> | our kernel: kernel_main |
   +--------+    +--------------------+    +-------------------------+
   firmware      switches the CPU into     receives BootInfo (incl.
   loads the     64-bit long mode, sets     the memory map), sets up
   bootloader    up paging, loads our       the CPU, starts the shell
                 kernel ELF into RAM
```

Inside `kernel_main` the startup sequence is:

```
   gdt::init()              load GDT + TSS (a safe stack for fatal faults)
        |
   interrupts::init_idt()   install the interrupt table (exceptions + IRQs)
        |
   PICS.initialize()        program the interrupt controller (timer, keyboard)
        |
   interrupts::enable()     allow interrupts to fire
        |
   shell.start()            print banner + prompt
        |
   loop { drain keys }      the REPL
```

---

## Architecture

```
   keyboard hardware                                      screen (VGA @ 0xb8000)
        | IRQ1 (interrupt)                                        ^
        v                                                         | bytes
  +-------------------+   scancode   +-------------------+        |
  |   interrupts.rs   |  ---------->  |     shell.rs      |  ------+
  |-------------------|   (queue)     |-------------------|
  | keyboard ISR:     |               | decode scancode,  |
  | read port 0x60,   |               | edit line buffer, |
  | push to queue     |               | dispatch command  |
  +-------------------+               +-------------------+
        ^                                   ^      |
        | drains queue                      |      | print!/println!
        |                                   |      v
  +-------------------+               +-------------------+
  |     main.rs       | --new Shell-->|   vga_buffer.rs   |
  | kernel_main:      |               | Writer @ 0xb8000  |
  | boot + REPL loop  |               +-------------------+
  +-------------------+
        ^
        | (safety net for fatal faults)
  +-------------------+
  |      gdt.rs       |
  +-------------------+
```

---

## Module map

| File                 | Kernel subsystem               | Responsibility                                                                 |
|----------------------|--------------------------------|--------------------------------------------------------------------------------|
| `src/main.rs`        | Kernel entry point             | `#![no_std]`/`#![no_main]`, boot sequence, REPL loop, the panic handler.       |
| `src/vga_buffer.rs`  | Screen driver                  | Write characters to VGA memory at `0xb8000`; provides `print!`/`println!`.     |
| `src/gdt.rs`         | GDT + TSS                      | A dedicated stack for the double-fault handler, so faults can't triple-fault.  |
| `src/interrupts.rs`  | IDT + PIC + keyboard ISR       | Install handlers; the keyboard interrupt reads scancodes into a queue.         |
| `src/shell.rs`       | Keyboard line discipline + REPL| Decode scancodes, edit the line buffer, parse + run commands.                  |

---

## How a keypress becomes a command

```
  1. You press 'h'
        |
        v
  2. Keyboard hardware raises IRQ1  ---------------------------+
why an interrupt? so the CPU doesn't have to poll the keyboard |
        |                                                      |
        v                                                      |
  3. CPU jumps to keyboard_interrupt_handler (interrupts.rs)   |
        - reads the scancode from port 0x60                    |
        - pushes it into SCANCODE_QUEUE                        |
        - acknowledges the PIC, returns                        |
        |                                                      |
        v                                                      |
  4. main loop calls interrupts::next_scancode() <-------------+
        - pops the scancode from the queue
        |
        v
  5. shell.feed_scancode(scancode)
        - pc-keyboard decodes it -> DecodedKey::Unicode('h')
        - 'h' is printable -> push into line buffer, echo to screen
        |
        v
  ... repeat for 'e','l','p', then Enter ...
        |
        v
  6. on Enter -> dispatch("help") -> cmd_help() -> prints the help text
        |
        v
  7. prompt() again
```

---

## The interrupt path (top half / bottom half)

A core kernel idea: **do almost nothing inside an interrupt handler.** Heavy
work (decoding keys, running commands, printing) is risky there — it can
deadlock on a lock the interrupted code already held. So we split the work:

```
   TOP HALF (interrupt handler, must be fast)        BOTTOM HALF (main loop)
   ------------------------------------------        -----------------------
   keyboard_interrupt_handler():                     loop {
     scancode = read port 0x60                          if let Some(sc) =
     SCANCODE_QUEUE.push(scancode)   --- queue --->        next_scancode() {
     PIC.notify_end_of_interrupt()                          shell.feed_scancode(sc)
   // returns immediately                                 } else {
                                                            enable_and_hlt()  // sleep
                                                          }
                                                       }
```

The queue is a fixed-size ring buffer (no heap). The consumer pops it with
interrupts **disabled**, which is what makes it safe on a single CPU: the ISR
can't run while we hold the lock, so they can never deadlock.

---

## Memory: VGA + the real memory map

**Output** is just memory writes. The screen is an 80x25 grid at physical
address `0xb8000`; each cell is two bytes:

```
   one screen cell (2 bytes):
   +-----------------+------------------------------+
   | ASCII character |  color: bg<<4 | fg           |
   |   (byte 0)      |        (byte 1)              |
   +-----------------+------------------------------+
   writing these two bytes makes a character appear instantly
```

**The `mem` command** reads the real map the bootloader discovered and handed
us in `BootInfo`. Example output in QEMU (128 MiB of RAM):

```
kernel> mem
physical memory map (from bootloader):
  0x000000000 - 0x000001000  FrameZero
  0x000001000 - 0x000005000  PageTable
  0x000005000 - 0x000015000  Bootloader
  0x000015000 - 0x000016000  BootInfo
  0x000016000 - 0x00001d000  Kernel
  ...
  0x000446000 - 0x007fe0000  Usable
  usable RAM: 128112 KiB
kernel>
```

---

## Commands

Each command is also a little lesson in a real kernel concept:

| Command          | What it does                                          | What it teaches                                |
|------------------|-------------------------------------------------------|------------------------------------------------|
| `help`           | List the available commands.                          | —                                              |
| `clear`          | Clear the screen.                                     | Writing blanks to VGA memory.                  |
| `echo <text>`    | Print the text back.                                  | Argument parsing.                              |
| `mem`            | Print the real physical memory map.                   | The bootloader's memory map (`BootInfo`).      |
| `regs`           | Dump CR0/CR2/CR3/CR4 + RFLAGS.                        | Control registers; paging root; interrupt flag.|
| `cpuid`          | Show CPU vendor and feature flags.                    | The `cpuid` instruction.                       |
| `uptime`         | Time since boot.                                      | The timer interrupt as a clock.                |
| `int3`           | Fire a breakpoint exception and **recover**.          | Exceptions are recoverable, unlike panics.     |
| `peek <hex> [n]` | Hex-dump `n` bytes from a memory address.             | Raw memory access; the page-fault handler.     |
| `reboot`         | Reset the machine (8042 controller, port 0x64).       | Port I/O; the legacy keyboard controller.      |
| `shutdown`       | Power off (ACPI ports + QEMU debug-exit).             | ACPI power management; I/O ports.              |
| `panic [msg]`    | Trigger a real kernel panic (red banner, then halt).  | The panic path; halting the CPU.               |

Line editing supported at the prompt: printable characters, **Backspace**, and
**Enter**.

Things to try:

```
kernel> regs            # see paging (CR3) and the interrupt flag
kernel> cpuid           # who made this CPU?
kernel> peek 0xb8000 32 # peek at the screen's own video memory
kernel> int3            # raise an exception... and survive it
kernel> uptime          # the timer has been ticking ~18 times a second
```

---

## Build & run

Prerequisites (already wired up by `rust-toolchain.toml`, but listed for
clarity):

```bash
# nightly + the components needed to build a custom bare-metal target
rustup component add rust-src llvm-tools-preview --toolchain nightly

# the tool that wraps the kernel in a bootloader and makes a disk image
cargo install bootimage

# QEMU, to actually run it
#   Debian/Ubuntu: sudo apt install qemu-system-x86
```

Then:

```bash
cargo build          # compile the kernel (and core, from source)
cargo bootimage      # produce a bootable disk image (.bin)
cargo run            # build the image AND launch it in QEMU
```

`cargo run` opens a QEMU window showing the banner and a `kernel>` prompt.
Type commands; try `help`, `mem`, `echo hi`, `panic`.

The bootable image is written to:
`target/x86_64-kernel/debug/bootimage-shell.bin` — you can also write it to a
USB stick and boot a real PC with it.

---

## The toolchain setup (and why each piece exists)

This kernel needs more than `Cargo.toml`. Here's what each config file does:

```
   rust-toolchain.toml   pins nightly + rust-src + llvm-tools-preview
                         (bare-metal needs unstable features)

   x86_64-kernel.json    custom compile target describing a bare x86-64 machine:
                         - os: none, panic: abort
                         - disable-redzone (unsafe when interrupts can fire)
                         - non-PIE, linked at a fixed address (bootloader needs this)
                         - soft-float / no SSE (see gotchas below)

   .cargo/config.toml    - target = our custom JSON
                         - build-std: compile `core` from source for the target
                         - runner = "bootimage runner" so `cargo run` boots QEMU

   Cargo.toml            - panic = "abort" in both profiles (can't unwind)
                         - the kernel crates: bootloader, x86_64, pic8259,
                           pc-keyboard, volatile, spin, lazy_static
```

---

## Gotchas we hit (and the fixes)

These are the real problems encountered bringing this up — worth knowing if you
extend it:

1. **`soft-float is incompatible with the ABI`** when disabling SSE in the
   target. Fix: add `"rustc-abi": "x86-softfloat"` to the target JSON so the
   whole target uses the soft-float ABI consistently.

2. **Bootloader panic: `failed to map page ... PageAlreadyMapped`** when using
   the built-in `x86_64-unknown-none` target. That target builds a
   position-independent executable linked at address 0, which the 0.9
   bootloader can't map. Fix: a custom target with
   `"relocation-model": "static"` and `"position-independent-executables": false`
   so the kernel links at a fixed high address.

3. **Invalid-opcode (#UD) interrupt storm / frozen prompt.** With SSE enabled in
   codegen but not enabled in hardware, the first SSE instruction faults
   forever. Fix: disable SSE in the target (`-mmx,-sse,+soft-float`) so the
   compiler never emits SSE — combined with gotcha #1.

4. **`.json target specs require -Zjson-target-spec`** on recent nightlies.
   Fix: `json-target-spec = true` under `[unstable]` in `.cargo/config.toml`.

---

## File layout

```
shell/
├── Cargo.toml            # crates + panic=abort profiles
├── rust-toolchain.toml   # nightly + rust-src + llvm-tools-preview
├── x86_64-kernel.json    # custom bare-metal compile target
├── .cargo/
│   └── config.toml       # build-std, target, QEMU runner
├── README.md             # this document
└── src/
    ├── main.rs           # entry point: boot sequence + REPL loop + panic handler
    ├── vga_buffer.rs      # screen driver: write to VGA memory; print! macros
    ├── gdt.rs            # GDT + TSS: safe stack for the double-fault handler
    ├── interrupts.rs     # IDT + PIC + keyboard ISR + scancode queue
    └── shell.rs          # keyboard decode, line editing, command dispatch
```
