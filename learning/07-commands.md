# Chapter 7 — The commands

**Real file:** `../src/shell.rs` (the command handlers)
**Goal:** understand each built-in, and the kernel concept it demonstrates.

🎯 **Milestone:** you understand what every command does and the kernel concept
behind it — and, most importantly, **you add your own command** and run it. This
is where you stop reading the project and start developing it.

Each command is a method on `Shell`. They're where the shell becomes a real
"debugging cockpit".

---

## `mem` — the real physical memory map

```rust
fn cmd_mem(&self) {
    println!("physical memory map (from bootloader):");
    let mut usable: u64 = 0;

    for region in self.memory_map.iter() {
        let start = region.range.start_addr();
        let end = region.range.end_addr();
        println!("  {:#012x} - {:#012x}  {:?}", start, end, region.region_type);

        if region.region_type == bootloader::bootinfo::MemoryRegionType::Usable {
            usable += end - start;
        }
    }
    println!("  usable RAM: {} KiB", usable / 1024);
}
```

- `self.memory_map` came from the bootloader via `BootInfo` (Chapter 1). This is
  the **genuine** map of physical RAM: which ranges are usable, which are
  reserved, which hold the kernel/bootloader/page tables.
- We sum the `Usable` regions to report total free RAM.
- **Concept:** on real hardware you don't *decide* your memory layout — the
  firmware tells you what exists, and you build on top of it.

---

## `regs` — CPU control registers

```rust
fn cmd_regs(&self) {
    use x86_64::registers::control::{Cr0, Cr2, Cr3, Cr4};
    use x86_64::registers::rflags::{self, RFlags};

    let cr3 = Cr3::read();
    println!("CPU registers:");
    println!("  CR0    = {:#018x}", Cr0::read().bits());
    println!("  CR2    = {:#018x}", Cr2::read().as_u64());
    println!("  CR3    = {:#018x}  (page table root)", cr3.0.start_address().as_u64());
    println!("  CR4    = {:#018x}", Cr4::read().bits());
    let flags = rflags::read();
    println!("  RFLAGS = {:#018x}", flags.bits());
    println!("    interrupts: {}",
        if flags.contains(RFlags::INTERRUPT_FLAG) { "enabled" } else { "disabled" });
}
```

- **Control registers** steer the CPU at the lowest level:
  - `CR0` — mode bits (protected mode, paging enabled, …).
  - `CR2` — the address of the last page fault (Chapter 4).
  - `CR3` — physical address of the **top-level page table**. This single
    register *is* "which address space am I in".
  - `CR4` — feature-enable bits.
  - `RFLAGS` — status flags; **bit 9** is the interrupt-enable flag.
- **Concept:** these registers are the dashboard of the CPU. `regs` lets you see
  paging is on and interrupts are enabled — live.

---

## `cpuid` — ask the CPU what it is

```rust
fn cmd_cpuid(&self) {
    use core::arch::x86_64::__cpuid;

    let leaf0 = __cpuid(0);
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());
    let vendor = core::str::from_utf8(&vendor).unwrap_or("?");

    println!("CPU vendor: {}", vendor);
    println!("max cpuid leaf: {}", leaf0.eax);

    let leaf1 = __cpuid(1);
    let edx = leaf1.edx;
    println!("features:");
    println!("  FPU   (x87 float)  : {}", bit(edx, 0));
    println!("  TSC   (timestamp)  : {}", bit(edx, 4));
    println!("  APIC  (local APIC) : {}", bit(edx, 9));
    println!("  SSE                : {}", bit(edx, 25));
    println!("  SSE2               : {}", bit(edx, 26));
}
```

with the helper:

```rust
fn bit(value: u32, n: u32) -> &'static str {
    if value & (1 << n) != 0 { "yes" } else { "no" }
}
```

- `cpuid` is a CPU instruction. You put a **leaf** number in EAX; it returns data
  in EAX/EBX/ECX/EDX.
  - Leaf 0: the 12-byte vendor string lives in EBX, then EDX, then ECX (that odd
    order is just how Intel defined it).
  - Leaf 1: feature bits — we test a few well-known ones in EDX.
- **Concept:** software discovers hardware capabilities at runtime instead of
  assuming them.

---

## `tsc` — the cycle-accurate counter

```rust
fn cmd_tsc(&self) {
    let cycles = unsafe { core::arch::x86_64::_rdtsc() };
    println!("timestamp counter: {} cycles since CPU reset", cycles);
}
```

- The **Time Stamp Counter** is a 64-bit register the CPU bumps once per clock
  cycle. `_rdtsc()` emits the `rdtsc` instruction to read it.
- It's the *highest-resolution* timer on the chip. Read it before and after some
  work, subtract, and you've measured that work in CPU cycles.
- **Concept:** contrast with `uptime`. `uptime` counts ~18 timer *interrupts* a
  second (coarse, but a real wall-clock-ish rate); `tsc` counts *cycles*
  (extremely fine, but you'd need the CPU frequency to turn it into seconds).

---

## `uptime` — using the timer tick counter

```rust
fn cmd_uptime(&self) {
    let ticks = interrupts::ticks();
    let tenths = ticks * 10 / interrupts::TIMER_HZ_X10;   // ticks -> 0.1s units
    println!("uptime: {} ticks  (~{}.{} seconds at ~18.2 Hz)",
        ticks, tenths / 10, tenths % 10);
}
```

- `interrupts::ticks()` reads the counter the timer interrupt has been bumping
  (Chapter 5).
- We avoid floating point (we disabled the FPU/SSE) by keeping the frequency
  ×10 and doing integer math.
- **Concept:** a hardware interrupt firing at a known rate *is* a clock.

---

## `sleep` — waiting without a scheduler

```rust
fn cmd_sleep(&self, args: &str) {
    let n: u64 = /* parse args */;
    let target = interrupts::ticks() + n;
    while interrupts::ticks() < target {
        x86_64::instructions::hlt();   // park the CPU until the next interrupt
    }
}
```

- A normal OS would put the task to sleep and run something else. We have no
  scheduler, so we *wait* — but the kernel-friendly way.
- `hlt` parks the CPU until the next interrupt. The timer fires ~18 times a
  second, so the CPU wakes, we re-check the tick count, and `hlt` again. This is
  the same idle trick the main loop uses (`enable_and_hlt`, Chapter 6) — far
  better than spinning hot.
- **Concept:** "doing nothing" efficiently is a real kernel skill. Busy-spinning
  wastes power; halting and waking on interrupts doesn't.

---

## `time` — reading the real-time clock (RTC)

```rust
fn read_cmos(reg: u8) -> u8 {
    let mut addr: Port<u8> = Port::new(0x70);   // select register...
    let mut data: Port<u8> = Port::new(0x71);   // ...then read its value
    unsafe { addr.write(reg); data.read() }
}
// seconds = read_cmos(0x00), minutes = 0x02, hours = 0x04, ...
```

- The **RTC** is a tiny battery-backed clock in the CMOS chip. You talk to it
  through two I/O ports: write a register number to `0x70`, read the value from
  `0x71`.
- The values come back in **BCD** — each nibble is one decimal digit, so `0x59`
  means *59*, not 89. We convert with `(v >> 4) * 10 + (v & 0x0f)`.
- We first wait for the "update in progress" bit (status register A, bit 7) to
  clear, so we don't read a half-updated time.
- **Concept:** real devices have their own little protocols. Here it's
  "select-then-read over two ports", plus a data format (BCD) you must decode.

---

## `colors` — the VGA palette

```rust
fn cmd_colors(&self) {
    for (color, name) in PALETTE {
        vga_buffer::print_colored("    ", Color::Black, color); // swatch
        vga_buffer::print_colored(name, color, Color::Black);   // name
        println!("  (code {})", color as u8);
    }
}
```

- Prints all 16 VGA text colors from Chapter 2 — a swatch (the color used as a
  *background*) and the color's name written *in* that color.
- It uses a small helper we added to `vga_buffer.rs`, `print_colored`, which
  temporarily swaps the writer's color, prints, then restores it. That's why the
  prompt goes back to yellow afterwards.
- **Concept:** that second byte of every screen cell — `bg << 4 | fg` — is the
  whole palette. Sixteen foregrounds × sixteen backgrounds, four bits each.

---

## `gdt` and `idt` — peek at the CPU's tables

```rust
fn cmd_gdt(&self) {
    let p = x86_64::instructions::tables::sgdt();   // "store GDT register"
    println!("  base  = {:#018x}", p.base.as_u64());
    println!("  limit = {} bytes", p.limit);
}
fn cmd_idt(&self) {
    let p = x86_64::instructions::tables::sidt();   // "store IDT register"
    println!("  base  = {:#018x}", p.base.as_u64());
    println!("  limit = {} bytes", p.limit);
}
```

- `sgdt`/`sidt` ask the CPU "where is the table, and how big is it?" — they read
  the GDTR/IDTR registers we loaded back in Chapters 3 and 4.
- The **limit** is `size - 1` in bytes. GDT entries are 8 bytes each (the TSS
  descriptor is 16); IDT entries are 16 bytes in long mode, so a full 256-vector
  IDT is `256 * 16 = 4096` bytes (`limit = 4095`).
- **Concept:** these registers are how the CPU *finds* the tables on every
  interrupt. `gdt`/`idt` let you confirm the structures from Chapters 3–4 are
  really loaded and where they live.

---

## `int3` — a recoverable exception

```rust
fn cmd_int3(&self) {
    println!("triggering a breakpoint exception (int3)...");
    x86_64::instructions::interrupts::int3();
    println!("...and we're back! the exception was handled and recovered.");
}
```

- `int3` executes the one-byte breakpoint instruction → the breakpoint handler
  (Chapter 4) runs, prints, and **returns**.
- Because it returns, the second `println!` runs. That's the lesson: an
  exception is a detour, not necessarily a death. Contrast with `panic`.

---

## `peek` — read raw memory

```rust
fn cmd_peek(&self, args: &str) {
    let mut parts = args.split_whitespace();

    let addr_str = parts.next().unwrap_or("");
    let addr_str = addr_str.strip_prefix("0x").unwrap_or(addr_str);
    let addr = match u64::from_str_radix(addr_str, 16) {
        Ok(value) => value,
        Err(_) => { println!("usage: peek <hex-address> [count]   e.g. peek 0xb8000 32"); return; }
    };

    let count: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(16).min(256);

    println!("memory at {:#x}:", addr);
    for row in 0..count.div_ceil(16) {
        let base = addr + row * 16;
        print!("  {:#012x}: ", base);
        for col in 0..16 {
            if row * 16 + col >= count { break; }
            let byte = unsafe { core::ptr::read_volatile((base + col) as *const u8) };
            print!("{:02x} ", byte);
        }
        println!();
    }
}
```

- Parses a hex address (with optional `0x`) and an optional byte count.
- `read_volatile` reads one byte at an arbitrary address. It's `unsafe` — but if
  the address is unmapped, the **page-fault handler** (Chapter 4) catches it and
  reports cleanly instead of rebooting.
- Try `peek 0xb8000 32` to read the screen's own video memory — you'll see
  `20 07 …` (space + light-gray-on-black) for blank cells.
- **Concept:** in a kernel, memory is just numbered bytes you can address
  directly. Powerful and dangerous.

---

## `poke` — the write side of `peek`

```rust
fn cmd_poke(&self, args: &str) {
    // parse <hex-address> and <hex-byte> ...
    unsafe { core::ptr::write_volatile(addr as *mut u8, value) };
    println!("wrote {:#04x} to {:#x}", value, addr);
}
```

- The counterpart to `peek`: instead of reading a byte from an address, it
  **writes** one. `write_volatile` makes sure the write actually happens.
- The fun demo ties straight back to Chapter 2: `poke 0xb8000 0x41` writes the
  byte `0x41` (`'A'`) into the very first screen cell — you'll see an `A` appear
  in the top-left corner. The screen really is just memory.
- Like `peek`, writing to an unmapped address faults into the page-fault handler
  (Chapter 4) instead of silently corrupting anything.
- **Concept:** memory is numbered bytes you can read *and* write directly. With
  no OS to mediate, "store to address X" is a one-instruction operation.

---

## `inb` / `outb` — talking to I/O ports directly

```rust
fn cmd_inb(&self, args: &str) {
    let mut port: Port<u8> = Port::new(port_num);
    let value = unsafe { port.read() };          // the `in` instruction
    println!("inb {:#x} = {:#04x}", port_num, value);
}
fn cmd_outb(&self, args: &str) {
    let mut port: Port<u8> = Port::new(port_num);
    unsafe { port.write(value) };                // the `out` instruction
}
```

- **I/O ports** are a separate address space from memory, reached with the `in`
  and `out` instructions (wrapped here by `Port::read`/`Port::write`). This is
  how the kernel already talks to hardware: the keyboard data port `0x60`
  (Chapter 5), the PIC, and `reboot`'s port `0x64`.
- These commands expose that directly so you can poke at hardware by hand:
  - `inb 0x64` — read the keyboard controller's **status** register (safe).
  - `outb 0x80 0x00` — write to the **POST** diagnostic port (harmless).
- ⚠️ `outb` is powerful: writing to the wrong port can confuse a device or even
  reset the machine. That's not a bug — it's what "bare metal" means. Read
  (`inb`) freely; write (`outb`) deliberately.
- **Concept:** under every nice driver is just `in`/`out` on a port number.
  `reboot` and `shutdown` are nothing more than the right `outb` to the right
  port.

---

## `reboot` and `shutdown` — power control via I/O ports

```rust
fn cmd_reboot(&self) -> ! {
    use x86_64::instructions::port::Port;
    println!("rebooting...");
    let mut port: Port<u8> = Port::new(0x64);   // 8042 keyboard controller
    unsafe { port.write(0xFEu8) };               // pulse the CPU reset line
    loop { x86_64::instructions::hlt(); }
}
```

```rust
fn cmd_shutdown(&self) -> ! {
    use x86_64::instructions::port::Port;
    println!("shutting down...");
    for port_addr in [0x604u16, 0xB004, 0x4004] {   // ACPI ports (vary by VM/firmware)
        let mut port: Port<u16> = Port::new(port_addr);
        unsafe { port.write(0x2000u16) };
    }
    let mut debug_exit: Port<u32> = Port::new(0xf4);  // QEMU isa-debug-exit fallback
    unsafe { debug_exit.write(0x10u32) };
    loop { x86_64::instructions::hlt(); }
}
```

- **Port I/O** is a separate address space from memory, accessed with special
  `in`/`out` instructions (`Port::read`/`Port::write` wrap them). Devices live
  here.
- `reboot`: pulsing the legacy keyboard controller (port `0x64`) with `0xFE`
  asserts the CPU reset line — an old but reliable PC trick.
- `shutdown`: power-off goes through **ACPI**, but the exact port differs across
  VMs/firmware, so we try the common ones, plus QEMU's `isa-debug-exit` device
  (port `0xf4`) as a reliable fallback when running under QEMU.
- Both return `!` and end in a halt loop in case the operation didn't take.

---

## `panic` — a real kernel panic

```rust
fn cmd_panic(&self, args: &str) -> ! {
    if args.is_empty() {
        panic!("manual panic triggered from the shell");
    } else {
        panic!("{}", args);
    }
}
```

- Calls `panic!`, which lands in the `#[panic_handler]` from Chapter 1: red
  banner, message, halt forever.
- **Concept:** unlike a recoverable exception, a panic is terminal — there's no
  OS to recover into.

---

## 🛠 Develop it: add your own command (walkthrough)

Time to extend the kernel yourself. We'll add a `rev <text>` command that prints
its argument reversed. It's small, but it touches every part of the dispatch
pipeline — exactly the pattern you'll reuse for bigger commands.

All three edits are in `../src/shell.rs`.

**Step 1 — register the command** in `dispatch` (add one arm to the `match`):

```rust
        "echo" => println!("{}", args),
        "rev"  => self.cmd_rev(args),     // <-- add this line
        "mem"  => self.cmd_mem(),
```

**Step 2 — write the handler** as a method on `Shell` (put it next to the other
`cmd_*` methods):

```rust
    /// `rev <text>` — print the argument with its bytes reversed.
    fn cmd_rev(&self, args: &str) {
        // No heap, so no String::chars().rev().collect(). We print byte by byte
        // from the end. (ASCII only — good enough for the shell's input.)
        for &byte in args.as_bytes().iter().rev() {
            print!("{}", byte as char);
        }
        println!();
    }
```

**Step 3 — document it** in `cmd_help` so `help` lists it:

```rust
        println!("  echo <text>     print the given text back");
        println!("  rev <text>      print the given text reversed");   // <-- add
```

**Step 4 — build and run:**

```bash
cargo run
```

```
kernel> rev hello kernel
lenrek olleh
kernel> help          # your command now appears in the list
```

That's the whole loop of kernel development: change `src/`, `cargo run`, observe.
You now know how to add features.

### More ideas to try on your own

The commands above (`tsc`, `colors`, `sleep`, `poke`, `inb`/`outb`, `time`,
`gdt`/`idt`) are all built in now — **read their source in `../src/shell.rs`**;
each is a worked example you can copy the pattern from. Here are fresh exercises,
all within the areas you already know (VGA, interrupts/timer, ports, CPU):

- **`fill <color>`** — fill the whole screen with one background color. Add a
  helper to `vga_buffer.rs` (model it on `clear_screen`) and parse a color name
  or 0–15. *Builds on:* VGA (Chapter 2).
- **`inw` / `outw`** — 16-bit port I/O, the `u16` versions of `inb`/`outb`. Just
  use `Port<u16>` instead of `Port<u8>`. *Builds on:* `inb`/`outb` above.
- **`peek` with ASCII** — extend `cmd_peek` to print an ASCII column beside the
  hex (printable bytes as-is, others as `.`), like a real hex dumper. *Builds
  on:* `peek` + string handling.
- **`scancodes`** — toggle a mode where the shell prints each raw scancode it
  receives instead of editing a line. Add a `bool` field to `Shell` and check it
  in `feed_scancode`. *Builds on:* the keyboard path (Chapters 5–6).
- **`beep <ticks>`** *(stretch)* — make the PC speaker beep: program PIT
  channel 2, then toggle port `0x61`, and use the tick counter to time it.
  *Builds on:* port I/O + the timer (Chapter 5). A classic bare-metal rite of
  passage.

> Stuck or made a mess? `git checkout src/shell.rs src/vga_buffer.rs` restores
> the originals.

---

## ✅ Checkpoint — the cockpit, end to end

Run the kernel and walk every command at least once:

```bash
cargo run
```

```
kernel> regs      # control registers; note "interrupts: enabled"
kernel> cpuid     # CPU vendor + feature flags
kernel> tsc       # CPU cycle counter (run it twice — it grows)
kernel> uptime    # grows over time (timer interrupt)
kernel> sleep 18  # pauses ~1 second, then prints "awake..."
kernel> time      # wall-clock time from the RTC chip
kernel> colors    # the 16-color VGA palette, in color
kernel> gdt       # where the GDT lives (Chapter 3)
kernel> idt       # where the IDT lives (Chapter 4)
kernel> mem       # the real physical memory map + total usable RAM
kernel> peek 0xb8000 32    # the screen's own memory
kernel> poke 0xb8000 0x41  # writes 'A' into the top-left screen cell
kernel> inb 0x64           # read the keyboard controller status port
kernel> rev racecar        # your new command
```

If each one prints sensible output, you've exercised every subsystem in the
kernel through its shell command — and added one of your own.

---

## What you learned

- `mem` reads the bootloader's real memory map; `regs`/`cpuid`/`tsc` read CPU
  state; `uptime`/`sleep` use the timer interrupt; `time` reads the RTC chip.
- `colors` shows the VGA palette; `gdt`/`idt` show where the boot-time tables
  live (Chapters 3–4).
- `int3` shows a recoverable exception; `panic` shows a fatal one.
- `peek`/`poke` do raw memory reads and writes (made safe by the page-fault
  handler); `inb`/`outb` do raw port I/O — the building block under every driver.
- `reboot`/`shutdown` are just the right `outb` to the right port.

**Next:** [Chapter 8 — Building & running it](08-build-and-run.md).
