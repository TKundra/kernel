# Learning the Mini Kernel Shell, chapter by chapter

This folder teaches the kernel the way you'd *build* it — from an empty
freestanding binary up to an interactive shell with a dozen commands. Each
chapter takes one piece of the real code (in `../src/`), shows it in small
chunks, and explains the meaning of every part and the OS concept behind it.

> Nothing here is compiled. These are **teaching copies** of the code in
> `../src/`. Read a chapter, then open the matching real file side by side.

## How to read this

Go in order — each chapter assumes the previous ones. The rough arc is:

```
   make it compile  ->  make it print  ->  make it not crash  ->
   make it react to hardware  ->  make it interactive  ->  make it useful
```

## Chapters

| # | Chapter | Real file | You'll learn |
|---|---------|-----------|--------------|
| 1 | [A freestanding binary](01-freestanding-binary.md) | `src/main.rs` | `#![no_std]`, `#![no_main]`, the panic handler, why there's no `main` |
| 2 | [Printing to the screen (VGA)](02-vga-text-output.md) | `src/vga_buffer.rs` | VGA text memory at `0xb8000`, `volatile`, building `println!` |
| 3 | [A safety net: GDT + TSS](03-gdt-and-tss.md) | `src/gdt.rs` | segments, the TSS, an emergency stack for fatal faults |
| 4 | [CPU exceptions & the IDT](04-cpu-exceptions-idt.md) | `src/interrupts.rs` | the interrupt table, breakpoint / double-fault / page-fault handlers |
| 5 | [Hardware interrupts & the keyboard](05-hardware-interrupts-keyboard.md) | `src/interrupts.rs` | the PIC, the timer tick, the keyboard ISR, the scancode queue |
| 6 | [The shell (REPL + line editing)](06-the-shell-repl.md) | `src/shell.rs` | decoding scancodes, the line buffer, parsing & dispatch |
| 7 | [The commands](07-commands.md) | `src/shell.rs` | `regs`, `cpuid`, `uptime`, `int3`, `peek`, `reboot`, `shutdown`, … |
| 8 | [Building & running it](08-build-and-run.md) | config files | the custom target, `build-std`, QEMU, and the bugs we hit |

## A mental model to keep in your head

```
   BIOS -> bootloader -> kernel_main
                              |
        +---------------------+---------------------+
        |          |          |          |          |
      gdt       interrupts   vga       shell     (the loop)
   (safe       (react to   (show     (read &      drains keys
    stack)      hardware)   text)     run cmds)   forever
```

When you finish, you'll understand every line of `../src/` and be able to add
your own commands and subsystems.
