# Chapter 0 — Setup & your first boot

**Goal:** install everything, **boot this kernel in QEMU**, and learn your way
around the project — all in about 10 minutes. From here on, every chapter ends
with you running the kernel, so we get that working *first*.

🎯 **Milestone:** a QEMU window opens showing a banner and a `kernel>` prompt
that you can type into. Once you've seen that, you've already done the hard part.

---

## 1. What you need installed

You need three things. Don't worry about *why* yet — Chapter 8 explains every
piece. For now, just install them.

### a) Rust (via rustup)

If you don't have Rust yet:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Check it:

```bash
rustc --version
cargo --version
```

### b) The nightly toolchain + components

This kernel uses unstable Rust features, so it needs **nightly** plus two extra
components. You do **not** need to run `rustup override` — the file
`../rust-toolchain.toml` already pins nightly automatically whenever you're
inside this project. You just need the components installed:

```bash
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
```

- `rust-src` — the source code of the `core` library. We compile it from source
  for our custom target (there's no prebuilt version).
- `llvm-tools-preview` — used when wrapping the kernel in a bootloader.

### c) `bootimage` and QEMU

```bash
# turns our compiled kernel into a bootable disk image
cargo install bootimage

# the PC emulator we boot that image in
#   Debian/Ubuntu:
sudo apt install qemu-system-x86
#   Fedora:        sudo dnf install qemu-system-x86
#   Arch:          sudo pacman -S qemu-system-x86
#   macOS (brew):  brew install qemu
```

Check QEMU is on your PATH:

```bash
qemu-system-x86_64 --version
```

> **If a command fails**, jump to [Troubleshooting](#troubleshooting) at the
> bottom — the usual culprits (missing `rust-src`, `bootimage` not on PATH) all
> have one-line fixes.

---

## 2. Build and run it — your first boot

From the project root (the folder with `Cargo.toml`):

```bash
cargo run
```

The **first** build is slow (a minute or two): Cargo is compiling the `core`
library from source for our bare-metal target, then `bootimage` is wrapping the
result in a bootloader and building a disk image. Later builds are fast.

When it finishes, a **QEMU window** opens showing:

```
=============================================
  mini kernel shell  -  type 'help' to begin
=============================================
kernel>
```

🎉 **That's your kernel running on a (virtual) bare machine — no Linux, no
Windows underneath it.** Click the window and type:

```
kernel> help
kernel> echo hello kernel
kernel> mem
```

To quit QEMU: close the window, or press **Ctrl-A** then **X** in the terminal
that launched it.

> ⚠️ **Don't panic if the prompt seems frozen at first** — click inside the
> QEMU window so it has keyboard focus. Keystrokes only reach the kernel when
> the window is focused.

---

## 3. The three build commands you'll use all course

You only need these three. You'll run the last one constantly:

```bash
cargo build      # just compile the kernel (catches code errors fast)
cargo bootimage  # compile + wrap into a bootable .bin disk image
cargo run        # do both, then launch it in QEMU   <-- your main command
```

The bootable image lands at
`target/x86_64-kernel/debug/bootimage-terminal.bin`. It's a *real* disk image —
you could write it to a USB stick and boot a physical PC with it.

---

## 4. A tour of the project

Here's the whole project. The five files in `src/` are the kernel; each gets its
own chapter. The four config files in the root are what make a bare-metal build
possible (Chapter 8).

```
kernel/
├── Cargo.toml            # crates we depend on + "abort on panic"      (Ch 8)
├── rust-toolchain.toml   # pin nightly + rust-src + llvm-tools         (Ch 8)
├── x86_64-kernel.json    # our custom bare-metal compile target        (Ch 8)
├── .cargo/config.toml    # build-std, default target, QEMU runner      (Ch 8)
└── src/
    ├── main.rs           # entry point, boot sequence, the REPL loop    (Ch 1, 6)
    ├── vga_buffer.rs     # screen output + print!/println! macros       (Ch 2)
    ├── gdt.rs            # emergency stack for fatal faults             (Ch 3)
    ├── interrupts.rs     # IDT, exceptions, PIC, timer, keyboard ISR    (Ch 4, 5)
    └── shell.rs          # line editing + command dispatch + commands   (Ch 6, 7)
```

The order the kernel boots is also the order of the chapters — that's not a
coincidence. `kernel_main` (in `src/main.rs`) does this, top to bottom:

```
   gdt::init()              load the GDT + TSS        (Ch 3)
   interrupts::init_idt()   install the IDT           (Ch 4)
   PICS.initialize()        program the PIC           (Ch 5)
   interrupts::enable()     allow interrupts to fire  (Ch 5)
   shell.start()            print banner + prompt     (Ch 6)
   loop { drain keys }      the REPL                  (Ch 6)
```

---

## How to follow the rest of the course

For each chapter:

1. **Open the matching file** in `../src/` beside the chapter (the chapter names
   it at the top).
2. **Read the chapter.** It explains the code in small pieces and the OS concept
   behind each.
3. **Do the Checkpoint** at the end: run `cargo run` and confirm you see the
   described behavior. This is how you *prove* you understood the subsystem.
4. **Experiment.** Change a number, rebuild, see what happens. You already have a
   working kernel — break it and fix it freely. (`git stash` or `git checkout
   .` restores the original code if you get stuck.)

---

## ✅ Checkpoint

Before moving on, make sure all of these worked:

- [ ] `cargo run` opened a QEMU window with the banner and `kernel>` prompt.
- [ ] Typing `help` listed a couple dozen commands.
- [ ] Typing `echo hi` printed `hi`.
- [ ] You can quit QEMU (close the window, or **Ctrl-A** then **X**).

If every box is checked, you have a working kernel and a working toolchain.
Everything else is understanding *how* and *why*.

**Next:** [Chapter 1 — A freestanding binary](01-freestanding-binary.md), where
we explain why a kernel can't look like a normal Rust program.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `error[E0463]: can't find crate for 'core'` / `can't find crate for 'compiler_builtins'` | `rust-src` isn't installed: `rustup component add rust-src --toolchain nightly` |
| `error: command 'bootimage' not found` (during `cargo run`) | `cargo install bootimage`, and make sure `~/.cargo/bin` is on your `PATH` |
| `qemu-system-x86_64: command not found` | Install QEMU (see step 1c) |
| Build uses **stable**, complains about unstable features | You're outside the project dir, so `rust-toolchain.toml` isn't applied. `cd` into the project root and retry. |
| `.json target specs require -Zjson-target-spec` | Your `.cargo/config.toml` is missing `json-target-spec = true` — it's already set in this repo (Chapter 8). |
| QEMU window opens but typing does nothing | Click inside the window to give it keyboard focus. |
| First build feels stuck | It's compiling `core` from source — give it 1–2 minutes the first time only. |
