# Chapter 7 — The commands

**Real file:** `../src/shell.rs` (the command handlers)
**Goal:** understand each built-in, and the kernel concept it demonstrates.

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

## Adding your own command (exercise)

1. Add an arm to the `match` in `dispatch` (Chapter 6).
2. Write a `cmd_yourthing(&self, args: &str)` method.
3. Add a line to `cmd_help`.
4. Rebuild and run.

Ideas: `tsc` (read the timestamp counter via `rdtsc`), `colors` (cycle the VGA
palette), `sleep <ticks>` (busy-wait on the tick counter), `echoback` history.

---

## What you learned

- `mem` reads the bootloader's real memory map; `regs`/`cpuid` read CPU state;
  `uptime` reads the timer tick counter.
- `int3` shows a recoverable exception; `panic` shows a fatal one.
- `peek` does raw memory reads (made safe by the page-fault handler).
- `reboot`/`shutdown` use port I/O to talk to the platform.

**Next:** [Chapter 8 — Building & running it](08-build-and-run.md).
