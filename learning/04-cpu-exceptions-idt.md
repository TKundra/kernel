# Chapter 4 — CPU exceptions & the IDT

**Real file:** `../src/interrupts.rs` (the exception half)
**Goal:** install the table that tells the CPU which function to run for each
exception, and write three handlers: breakpoint, double fault, page fault.

🎯 **Milestone:** you can make the CPU raise an exception on purpose (`int3`) and
watch the kernel **handle it and keep running** — proving an exception is a
detour, not a crash.

---

## What an interrupt is

An **interrupt** is the CPU pausing whatever it's doing, jumping to a handler
function, then resuming. Two sources:

- **CPU exceptions** — the CPU itself raises them: a breakpoint instruction, a
  divide-by-zero, a page fault, etc. (interrupt numbers 0–31).
- **Hardware interrupts** — devices raise them: timer, keyboard, etc. (Chapter 5).

The CPU finds the right handler in the **Interrupt Descriptor Table (IDT)**: a
table mapping each interrupt number (0–255) to a function pointer.

---

## A special calling convention

Interrupt handlers can't be normal functions — when an interrupt fires, the CPU
has pushed extra state and expects a special `iret` instruction to return. Rust
gives us a calling convention for this, but it's unstable, so `main.rs` opts in:

```rust
#![feature(abi_x86_interrupt)]
```

Then handlers are written as `extern "x86-interrupt" fn(...)`.

---

## Building the IDT

```rust
lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // CPU exceptions:
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);   // <- Chapter 3!
        }

        // Hardware interrupts (Chapter 5):
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);

        idt
    };
}

pub fn init_idt() {
    IDT.load();
}
```

- `InterruptDescriptorTable` has named fields for the standard exceptions
  (`breakpoint`, `page_fault`, `double_fault`, …) and is indexable (`idt[33]`)
  for the hardware interrupt numbers.
- `set_stack_index(DOUBLE_FAULT_IST_INDEX)` is the payoff from Chapter 3: it
  tells the CPU "run the double-fault handler on emergency stack slot 0". This
  is `unsafe` because a bad index would itself cause faults.
- `init_idt()` (called from `kernel_main`) loads the table into the CPU.

---

## Handler 1 — breakpoint (recoverable)

```rust
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}
```

- A breakpoint (`int3`) is **recoverable**: we print the saved CPU state and
  simply **return**, and execution continues right where it left off.
- `stack_frame` is what the CPU pushed: the instruction pointer, code segment,
  flags, stack pointer. `{:#?}` pretty-prints it.
- This is the handler our `int3` command relies on to demonstrate that
  exceptions are not the same as crashes.

---

## Handler 2 — double fault (not recoverable)

```rust
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}
```

- Note the `-> !`: a double fault means something already went badly wrong, so
  we can't return. We turn it into a kernel panic.
- This handler runs on the **emergency stack** from Chapter 3, so it works even
  if the normal stack is destroyed. That's what prevents a silent reboot.
- Double faults carry an error code (always 0 here), so the signature has a
  second parameter.

---

## Handler 3 — page fault

```rust
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    println!("EXCEPTION: PAGE FAULT");
    println!("  accessed address: {:?}", Cr2::read());
    println!("  error code: {:?}", error_code);
    panic!("unhandled page fault\n{:#?}", stack_frame);
}
```

- A **page fault** happens when code touches memory that isn't mapped (or isn't
  allowed). The CPU leaves the offending address in the **`CR2`** register, so
  we read and print it.
- The `error_code` flags *why* (was it a read or write? present page or not?).
- We can't usefully continue (the instruction would just fault again), so we
  report and panic.
- This is what makes our `peek` command **safe to play with**: poke a bad
  address and you get a clear message, not a mysterious reboot.

---

## Trying it: the `int3` command

Later, the shell's `int3` command does:

```rust
println!("triggering a breakpoint exception (int3)...");
x86_64::instructions::interrupts::int3();   // raises the breakpoint exception
println!("...and we're back!");             // we DID return — proof it recovered
```

The fact that the second line prints proves the handler ran and returned.

---

## ✅ Checkpoint — survive an exception

Run the kernel and fire a breakpoint exception on demand:

```bash
cargo run
```

```
kernel> int3
```

You should see all three lines:

```
triggering a breakpoint exception (int3)...
EXCEPTION: BREAKPOINT
InterruptStackFrame { ... instruction_pointer ... }
...and we're back! the exception was handled and recovered.
```

The last line is the whole point: the handler **returned**, and execution
continued. Compare with `kernel> panic` (Chapter 1), which never returns. Same
machinery (the IDT), opposite outcome — recoverable vs fatal.

You can also see the table itself:

```
kernel> idt
IDTR:
  base  = 0x...
  limit = 4095 bytes (256 entries)
```

That's `sidt` reading back the IDTR register `init_idt()` loaded. The limit is
`4095` because each of the 256 vectors is a 16-byte descriptor in 64-bit mode
(`256 * 16 = 4096`, and the limit is size − 1).

> Want to see the page-fault handler too? Use `peek` (Chapter 7) to read a high
> address far beyond installed RAM — e.g. `kernel> peek 0xffff800000000000 8`.
> Touching unmapped memory raises a page fault, and you get a clean
> `EXCEPTION: PAGE FAULT` report with the faulting address (then a panic),
> instead of a silent reboot — precisely because Chapter 3's emergency stack and
> this chapter's handler are in place.

---

## What you learned

- The IDT maps interrupt numbers to handlers; `set_handler_fn` fills it in.
- Handlers use the `extern "x86-interrupt"` ABI (unstable, opted in via a
  feature flag).
- Some exceptions are recoverable (breakpoint → return), some are fatal
  (double fault → panic, on the emergency stack).
- The page-fault handler reads the faulting address from `CR2` and makes raw
  memory pokes safe to experiment with.

**Next:** [Chapter 5 — Hardware interrupts & the keyboard](05-hardware-interrupts-keyboard.md).
