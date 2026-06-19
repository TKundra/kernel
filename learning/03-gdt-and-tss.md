# Chapter 3 — A safety net: the GDT and TSS

**Real file:** `../src/gdt.rs`
**Goal:** give fatal faults a guaranteed-good stack, so a crash prints an error
instead of silently rebooting the machine.

---

## The danger we're preventing

When the CPU hits a fault, it tries to call the handler — which means **pushing
data onto the stack**. But what if the fault *was* a stack problem (e.g. the
stack overflowed into unmapped memory)? Then the push fails, which raises a
second fault: a **double fault**. If handling *that* also fails, the CPU gives
up and **triple-faults** → the machine instantly reboots, with no error message.

The fix: give the double-fault handler its **own, separate stack** that is
always valid. Then even if the normal stack is wrecked, the handler can run and
tell us what happened.

The CPU finds that emergency stack through a chain of structures:

```
   IDT entry for "double fault"
        |  "use IST slot 0"
        v
   TSS (Task State Segment): holds a table of emergency stacks (the IST)
        |  registered via
        v
   GDT (Global Descriptor Table): the table the CPU loads at boot
```

So we build the GDT and TSS here; Chapter 4 points the double-fault handler at
the stack.

---

## The emergency stack, inside a TSS

```rust
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;          // 20 KiB
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            let stack_end = stack_start + STACK_SIZE as u64;
            stack_end                                     // x86 stacks grow DOWN
        };
        tss
    };
}
```

Line by line:

- `interrupt_stack_table` (IST) is a small array of stack pointers inside the
  TSS. We fill slot `0` with our emergency stack.
- `static mut STACK: [u8; 20 KiB]` reserves the actual bytes, baked into the
  kernel image. (No heap exists, so a `static` array is how we "allocate" it.)
- `&raw const STACK` takes a **raw pointer** to that static. We use a raw
  pointer rather than a normal reference because Rust 2024 forbids references to
  `static mut` (they'd be unsound if two parts of the code aliased it).
- x86 stacks grow **downward** (from high addresses to low), so the CPU wants
  the **top** (highest address). That's why we add `STACK_SIZE` to get
  `stack_end` and store *that*.

---

## The GDT: register a code segment and the TSS

```rust
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss_selector  = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, tss_selector })
    };
}
```

- In 64-bit mode, segmentation is mostly vestigial, but the CPU still requires a
  valid **code segment** and a **TSS** to be registered. We add both.
- `add_entry` hands back a **selector** — basically an index into the GDT. We
  keep them in `Selectors` so `init` can load them into the CPU.

---

## Loading it into the CPU

```rust
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();                       // tell the CPU where the GDT is
    unsafe {
        CS::set_reg(GDT.1.code_selector);   // switch to our code segment
        load_tss(GDT.1.tss_selector);       // tell the CPU where the TSS is
    }
}
```

- `GDT.0.load()` points the CPU's GDT register at our table.
- `CS::set_reg(...)` reloads the **code segment register** with our descriptor.
- `load_tss(...)` tells the CPU which descriptor is the TSS, so it knows where
  to find the emergency stacks.
- This is `unsafe` because loading wrong values here would crash the machine —
  we're touching the CPU's most fundamental state.

`kernel_main` calls `gdt::init()` **first**, before interrupts, so the safety
net is in place before anything can fault.

---

## What you learned

- A fault while the stack is broken escalates to a double fault, then a triple
  fault (instant reboot).
- The TSS holds emergency stacks (the IST); the GDT registers the TSS.
- We reserve a 20 KiB static stack and hand its **top** address to IST slot 0.
- `gdt::init()` loads the GDT, the code segment, and the TSS into the CPU.

**Next:** [Chapter 4 — CPU exceptions & the IDT](04-cpu-exceptions-idt.md), where
we actually install handlers (including the double-fault handler that uses this
stack).
