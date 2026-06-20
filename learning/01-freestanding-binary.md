# Chapter 1 — A freestanding binary

**Real file:** `../src/main.rs`
**Goal:** understand why a kernel can't look like a normal Rust program, and the
three things every freestanding binary needs.

🎯 **Milestone:** you can explain `#![no_std]`, `#![no_main]`, and the panic
handler — and you'll *see* the panic handler fire by running the `panic` command.

> You should have already booted the kernel in [Chapter 0](00-setup.md). If not,
> do that first — this chapter explains the file you just ran.

---

## The problem

A normal Rust program starts like this:

```rust
fn main() {
    println!("Hello, world!");
}
```

But `main` and `println!` both depend on an **operating system**:

- `println!` asks the OS to write to a file (stdout).
- Before `main` even runs, the Rust runtime (and the C runtime under it) does
  setup that assumes an OS is present.

A kernel **is** the thing an OS is made of. There's nothing underneath it. So we
have to remove both assumptions.

---

## 1. `#![no_std]` — drop the standard library

```rust
#![no_std]
```

`std` assumes an OS (files, threads, heap, networking…). We can't use it. What
we *can* use is **`core`**: the part of the standard library with no OS
dependencies — `Option`, `Result`, `Iterator`, slices, `for` loops, formatting,
etc. Most of Rust still works; we just lose `Vec`, `String`, `println!`, and
friends (we'll rebuild `println!` ourselves in Chapter 2).

---

## 2. `#![no_main]` — there is no `main`

```rust
#![no_main]
```

The normal entry point is `main`, but it's called *by* the runtime we just
removed. Instead, the **bootloader** will jump straight to an entry point we
define. We declare it with a macro from the `bootloader` crate:

```rust
use bootloader::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // ... bring up the machine, then run forever ...
}
```

- `entry_point!(kernel_main)` generates the low-level `_start` symbol the
  bootloader looks for, and checks that our function has the right signature.
- `boot_info: &'static BootInfo` is data the bootloader hands us — most
  importantly the **physical memory map** (we use it in the `mem` command).
- The return type is `!` ("never"). A kernel never returns — there's nothing to
  return *to*. It runs until you power off.

---

## 3. The panic handler — required, and it's our "kernel panic"

Every Rust program needs code that runs when something `panic!`s. Normally
`std` provides it. With `no_std` we must write it ourselves:

```rust
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    vga_buffer::set_panic_color();         // switch to white-on-red
    println!("\n*** KERNEL PANIC ***");
    println!("{}", info);                  // message + file:line

    loop {
        x86_64::instructions::hlt();        // park the CPU forever
    }
}
```

- `#[panic_handler]` marks this as *the* function the compiler calls on panic.
- It returns `!` too — a kernel panic is the end of the road. There's no OS to
  catch us, so we print the reason and stop.
- `hlt` halts the CPU until the next interrupt; the surrounding `loop` means we
  never resume real work. (We can't just `return`.)

This is why our `panic` *command* is a real kernel panic: it calls `panic!`,
which lands here, paints the screen red, and halts.

---

## Why we also need "abort on panic"

Normally a panic *unwinds* the stack (running destructors). Unwinding needs
runtime support we don't have. So we tell Cargo to just **abort** instead — in
`Cargo.toml`:

```toml
[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
```

Without this, the compiler would demand an "eh_personality" function used for
unwinding, and we'd get a build error.

---

## The shape of `main.rs`

Putting the skeleton together (details filled in by later chapters):

```rust
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]   // needed for interrupt handlers (Chapter 4)

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};

mod gdt;
mod interrupts;
mod shell;
mod vga_buffer;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // Chapters 2-5 set up the machine here.
    // Chapter 6 starts the shell and loops forever.
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    loop { x86_64::instructions::hlt(); }
}
```

---

## ✅ Checkpoint — see the panic handler fire

The whole kernel already builds, so let's prove *this chapter's* code works: the
`#[panic_handler]`. Run the kernel and trigger a panic from the shell:

```bash
cargo run
```

```
kernel> panic boom
```

You should see the screen turn into a **white-on-red** kernel panic:

```
*** KERNEL PANIC ***
panicked at 'boom', src/shell.rs:...
```

…and the machine **halts** — the prompt never comes back. That is the `-> !`
("never returns") in action: there's no OS to recover into, so we print the
reason and stop the CPU. Close QEMU and reopen it to recover.

> Want to see the bootloader call `kernel_main`? Open `src/main.rs` and confirm
> there is no `fn main` anywhere — only `entry_point!(kernel_main)`.

---

## What you learned

- A kernel can't use `std` (`#![no_std]`) or the normal entry point
  (`#![no_main]`); the bootloader calls our `kernel_main` instead.
- `kernel_main` and the panic handler both return `!` — kernels don't return.
- We must provide a `#[panic_handler]`, and build with `panic = "abort"`.

**Next:** [Chapter 2 — Printing to the screen](02-vga-text-output.md), where we
rebuild `println!` from scratch by writing to video memory.
