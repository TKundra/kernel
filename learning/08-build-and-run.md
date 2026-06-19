# Chapter 8 — Building & running it

**Real files:** `../Cargo.toml`, `../x86_64-kernel.json`, `../.cargo/config.toml`,
`../rust-toolchain.toml`
**Goal:** understand how a bare-metal kernel is compiled and booted — and the
real bugs we hit getting there.

---

## Why a kernel needs special build setup

A normal `cargo build` produces a Linux (or Windows/macOS) program linked
against that OS. Our kernel targets **no OS at all**, so we have to tell the
toolchain four things:

1. Compile for a **bare-metal target**, not the host OS.
2. Build the `core` library **from source** (there's no prebuilt one for our
   target).
3. Don't unwind on panic — **abort**.
4. After building, wrap the kernel in a **bootloader** and run it in **QEMU**.

Each config file handles part of this.

---

## `rust-toolchain.toml` — pin nightly + components

```toml
[toolchain]
channel = "nightly"
components = ["rust-src", "llvm-tools-preview"]
```

- **nightly** is required: we use unstable features (`abi_x86_interrupt`,
  `build-std`).
- `rust-src` — the source code of `core`, needed to compile it for our target.
- `llvm-tools-preview` — tools the bootloader build uses.

Just being in this folder selects this toolchain automatically.

---

## `x86_64-kernel.json` — the custom target

This describes a bare x86-64 machine to the compiler/linker:

```json
{
    "llvm-target": "x86_64-unknown-none",
    "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
    "arch": "x86_64",
    "target-endian": "little",
    "target-pointer-width": 64,
    "target-c-int-width": 32,
    "os": "none",
    "executables": true,
    "linker-flavor": "ld.lld",
    "linker": "rust-lld",
    "panic-strategy": "abort",
    "disable-redzone": true,
    "relocation-model": "static",
    "position-independent-executables": false,
    "rustc-abi": "x86-softfloat",
    "features": "-mmx,-sse,+soft-float"
}
```

The non-obvious fields, and **why each one is here** (these are the bug fixes):

- `"os": "none"` — no operating system.
- `"linker": "rust-lld"` — use Rust's bundled linker, not the host's C linker
  (which would try to link against the system libc).
- `"disable-redzone": true` — the "red zone" is a stack optimization that an
  interrupt can silently corrupt. Unsafe when interrupts fire at any time.
- `"relocation-model": "static"` + `"position-independent-executables": false`
  — link the kernel at a **fixed address**, not as a position-independent
  executable. The 0.9 bootloader can't load a PIE linked at address 0
  *(this fixed the "failed to map page ... PageAlreadyMapped" bootloader panic)*.
- `"features": "-mmx,-sse,+soft-float"` — **don't** use the SSE float registers.
  The bootloader doesn't enable SSE in hardware, so if the compiler emitted SSE
  instructions, the first one would raise an invalid-opcode fault — forever
  *(this fixed an exception storm that froze the shell at the prompt)*.
- `"rustc-abi": "x86-softfloat"` — tells rustc the whole target uses the
  soft-float ABI, so disabling SSE is consistent *(without this, modern rustc
  errors: "soft-float is incompatible with the ABI")*.

---

## `.cargo/config.toml` — wire it together

```toml
[build]
target = "x86_64-kernel.json"

[unstable]
build-std = ["core", "compiler_builtins"]
build-std-features = ["compiler-builtins-mem"]
json-target-spec = true

[target.'cfg(target_os = "none")']
runner = "bootimage runner"
```

- `target = "x86_64-kernel.json"` — build for our custom target by default.
- `build-std` — compile `core` (and `compiler_builtins`) from source for it.
- `build-std-features = ["compiler-builtins-mem"]` — provides `memcpy`/`memset`/
  etc. that the compiler assumes exist.
- `json-target-spec = true` — recent nightlies require opting in to JSON target
  files *(fixed: ".json target specs require -Zjson-target-spec")*.
- `runner = "bootimage runner"` — makes `cargo run` hand the kernel to
  `bootimage`, which boots it in QEMU.

---

## `Cargo.toml` — dependencies, panic, QEMU args

```toml
[dependencies]
bootloader  = "0.9.23"   # boots us; provides the memory map
volatile    = "0.2.6"    # un-optimizable hardware writes (Chapter 2)
spin        = "0.5.2"    # spinlocks (no scheduler, so no sleeping mutex)
x86_64      = "0.14.2"   # safe wrappers for IDT/GDT/ports/instructions
pic8259     = "0.10.1"   # the interrupt controller (Chapter 5)
pc-keyboard = "0.7.0"    # scancode -> key decoding (Chapter 6)
lazy_static = { version = "1.0", features = ["spin_no_std"] }  # runtime-init statics

[profile.dev]
panic = "abort"          # can't unwind without an OS

[profile.release]
panic = "abort"

[package.metadata.bootimage]
run-args = ["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]   # so `shutdown` can exit QEMU
```

---

## Building and running

```bash
cargo build      # compiles the kernel (and core, from source)
cargo bootimage  # wraps it in a bootloader -> a bootable .bin disk image
cargo run        # does both, then launches it in QEMU
```

- The bootable image lands at
  `target/x86_64-kernel/debug/bootimage-shell.bin`.
- `cargo run` opens a QEMU window with the banner and a `kernel>` prompt.
- That `.bin` is a real disk image — you could write it to a USB stick and boot
  an actual PC with it.

---

## The boot chain, end to end

```
   QEMU/BIOS
      | loads
   bootloader (the crate, built by `bootimage`)
      | switches CPU to 64-bit mode, sets up paging,
      | loads our kernel ELF, builds the memory map
      v
   _start (generated by entry_point!)
      v
   kernel_main(boot_info)
      | gdt::init() -> interrupts::init_idt() -> PICS.initialize() -> enable()
      | shell.start()
      v
   loop { drain scancodes -> shell }   (forever)
```

---

## The four bugs we hit (a debugging story)

Worth internalizing — these are *classic* bare-metal pitfalls:

| Symptom | Cause | Fix |
|---------|-------|-----|
| `.json target specs require -Zjson-target-spec` | newer nightly gate | `json-target-spec = true` |
| bootloader panic: `PageAlreadyMapped` at low memory | built-in target builds a PIE linked at address 0 | custom target, `relocation-model: static`, non-PIE |
| invalid-opcode storm, frozen prompt | SSE in codegen but not enabled in hardware | `-sse,+soft-float` |
| `soft-float is incompatible with the ABI` | inconsistent float ABI | `rustc-abi: x86-softfloat` |

---

## What you learned

- Bare-metal builds need a custom target, `build-std`, `panic = abort`, and a
  QEMU runner.
- Each exotic target flag exists to fix a concrete problem (PIE, SSE, ABI).
- `cargo run` = build kernel → wrap in bootloader → boot in QEMU.

---

## You finished!

You now understand every file in `../src/`:

```
   main.rs       entry point, REPL loop, panic handler        (Ch 1, 6)
   vga_buffer.rs screen output + print! macros                (Ch 2)
   gdt.rs        emergency stack for fatal faults             (Ch 3)
   interrupts.rs IDT, exceptions, PIC, timer, keyboard ISR    (Ch 4, 5)
   shell.rs      line editing + command dispatch + commands   (Ch 6, 7)
```

Good next projects: a **serial port** driver (a real log + automated testing), a
**heap allocator** (then `Vec`/`String` and command history), or paging
experiments you can drive from new shell commands.
