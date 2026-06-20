# Learn this kernel by building and running it

This folder is a **hands-on course**. By the end you will have booted a real
bare-metal x86-64 kernel, understood every line of it, and added your own
command to it.

The golden rule of this course: **you run the kernel after every chapter.** You
never read more than a few pages without seeing something happen on screen. That
is the difference between "reading about a kernel" and "learning to build one".

---

## Start here

👉 **[Chapter 0 — Setup & your first boot](00-setup.md)**

Do not skip it. Chapter 0 installs the tools, gets the kernel **booting in
QEMU on your machine in about 10 minutes**, and gives you a tour of the files.
Every chapter after it assumes you can already build and run.

If you only have 10 minutes today, do Chapter 0 and stop. You will have a
working kernel and you can come back for the "why" later.

---

## How this course works

The kernel already exists in full in `../src/`. Rather than make you type 1500
lines blind, the course works like this:

1. **Chapter 0** gets the *whole* kernel building and running, so you always
   have a working machine to poke at.
2. **Each later chapter takes one subsystem** (the screen, the keyboard, the
   shell…), explains the code in `../src/`, and the OS concept behind it.
3. **Each chapter ends with a Checkpoint**: a command to run and the exact
   output you should see, so that subsystem becomes real and verifiable — not
   just words on a page.

Every chapter follows the same shape:

```
  🎯 Milestone   — what you will understand / be able to do after this chapter
  ── the concept and the code, in small pieces ──
  ✅ Checkpoint  — run the kernel, do X, confirm you see Y
  📝 What you learned + a link to the next chapter
```

> The teaching snippets are lightly trimmed for clarity. The real, complete code
> is in `../src/` — keep the matching file open beside each chapter.

---

## The roadmap

Read in order. Each chapter builds on the previous one, in the order the kernel
itself boots:

```
   make it boot  ->  make it print  ->  make it not crash  ->
   make it react to hardware  ->  make it interactive  ->  make it useful
   ->  understand how it all builds  ->  extend it yourself
```

| # | Chapter | Real file | After this chapter you can… |
|---|---------|-----------|------------------------------|
| 0 | [Setup & first boot](00-setup.md) | all config | **build and run the kernel in QEMU** |
| 1 | [A freestanding binary](01-freestanding-binary.md) | `src/main.rs` | explain `#![no_std]`, `#![no_main]`, the panic handler, why there's no `main` |
| 2 | [Printing to the screen (VGA)](02-vga-text-output.md) | `src/vga_buffer.rs` | make text and colors appear by writing to `0xb8000` |
| 3 | [A safety net: GDT + TSS](03-gdt-and-tss.md) | `src/gdt.rs` | give fatal faults an emergency stack so they can't silently reboot |
| 4 | [CPU exceptions & the IDT](04-cpu-exceptions-idt.md) | `src/interrupts.rs` | install handlers; recover from a breakpoint with `int3` |
| 5 | [Hardware interrupts & the keyboard](05-hardware-interrupts-keyboard.md) | `src/interrupts.rs` | type at the prompt (the keyboard driver) |
| 6 | [The shell (REPL + line editing)](06-the-shell-repl.md) | `src/shell.rs` | turn keystrokes into edited lines and run commands |
| 7 | [The commands](07-commands.md) | `src/shell.rs` | use `regs`, `cpuid`, `uptime`, `peek`, … and **add your own** |
| 8 | [Building & running it](08-build-and-run.md) | config files | explain every build flag and the bugs they fix |

---

## A mental model to keep in your head

This is what you are building. Keep coming back to this picture:

```
   BIOS -> bootloader -> kernel_main
                              |
        +---------------------+---------------------+
        |          |          |          |          |
      gdt       interrupts   vga       shell     (the loop)
   (safe       (react to   (show     (read &      drains keys
    stack)      hardware)   text)     run cmds)   forever
```

---

## When you finish

You will be able to build the kernel from scratch, explain every file in
`../src/`, and extend it. Chapter 7 walks you through **adding your own shell
command**, and Chapter 8 ends with bigger projects to try next (a serial-port
driver, a heap allocator so you get `Vec`/`String`, paging experiments).
