# Chapter 5 — Hardware interrupts & the keyboard

**Real file:** `../src/interrupts.rs` (the hardware half)
**Goal:** get the PIC delivering timer and keyboard interrupts, and build a
keyboard driver that hands keystrokes to the shell without deadlocking.

---

## The PIC: how devices reach the CPU

Devices don't poke the CPU directly. They go through the **PIC** (Programmable
Interrupt Controller — the legacy 8259). It collects device signals (IRQs) and
raises them to the CPU one at a time.

Problem: by default the PIC uses interrupt numbers 0–15, which **collide** with
CPU exceptions (0–31). So the first thing we do is **remap** the PIC to numbers
32–47:

```rust
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;   // 40

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
```

After remapping: **timer = 32**, **keyboard = 33**. We name them:

```rust
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,    // 32
    Keyboard,                // 33 (next value)
}

impl InterruptIndex {
    fn as_u8(self) -> u8 { self as u8 }
    fn as_usize(self) -> usize { usize::from(self.as_u8()) }
}
```

`kernel_main` initializes and enables it all:

```rust
interrupts::init_idt();
unsafe { interrupts::PICS.lock().initialize() };       // program the PIC
x86_64::instructions::interrupts::enable();            // let interrupts fire
```

> **The golden rule of PIC interrupts:** every hardware interrupt handler MUST
> send an "end of interrupt" (EOI) signal when done, or the PIC will never send
> that interrupt again.

---

## The timer interrupt (also our clock)

The PIC's timer fires ~18.2 times a second. We don't strictly need it, but if we
*don't* handle it, the unhandled interrupt would fault. So we handle it — and
make it useful by counting ticks (this powers the `uptime` command):

```rust
static TICKS: AtomicU64 = AtomicU64::new(0);
pub const TIMER_HZ_X10: u64 = 182;     // 18.2 Hz, stored x10 to avoid floats

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}
```

- `AtomicU64` lets the handler bump the counter without a lock — safe to touch
  from an interrupt.
- `notify_end_of_interrupt` is the mandatory EOI.

---

## The keyboard, part 1: the interrupt handler ("top half")

A core kernel principle: **do as little as possible inside an interrupt
handler.** Heavy work there is slow and risks deadlock. So we split the keyboard
work into two halves.

The **top half** is the handler. It does the bare minimum: read the one scancode
the keyboard has for us, drop it in a queue, send EOI, return.

```rust
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);                 // PS/2 data port
    let scancode: u8 = unsafe { port.read() };      // MUST read it

    SCANCODE_QUEUE.lock().push(scancode);

    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}
```

- Port **`0x60`** is where the keyboard controller puts the scancode. We *must*
  read it, or the controller won't send the next interrupt.
- A **scancode** is a raw code for "key X was pressed/released" — not a
  character yet. Decoding into letters happens later, in the shell (Chapter 6).

---

## The keyboard, part 2: the scancode queue (the hand-off)

We need a buffer between the top half (producer) and the main loop (consumer).
With no heap, it's a fixed-size **ring buffer**:

```rust
const QUEUE_CAPACITY: usize = 256;

struct ScancodeQueue {
    buffer: [u8; QUEUE_CAPACITY],
    head: usize,   // read index
    tail: usize,   // write index
    count: usize,  // how many are stored
}

impl ScancodeQueue {
    const fn new() -> Self {
        ScancodeQueue { buffer: [0; QUEUE_CAPACITY], head: 0, tail: 0, count: 0 }
    }

    fn push(&mut self, value: u8) {
        if self.count == QUEUE_CAPACITY { return; }  // full: drop it
        self.buffer[self.tail] = value;
        self.tail = (self.tail + 1) % QUEUE_CAPACITY;
        self.count += 1;
    }

    fn pop(&mut self) -> Option<u8> {
        if self.count == 0 { return None; }
        let value = self.buffer[self.head];
        self.head = (self.head + 1) % QUEUE_CAPACITY;
        self.count -= 1;
        Some(value)
    }
}

static SCANCODE_QUEUE: Mutex<ScancodeQueue> = Mutex::new(ScancodeQueue::new());
```

- `% QUEUE_CAPACITY` makes the indices wrap around — that's the "ring".
- If the queue fills up we **drop** the byte rather than block. Losing a
  keystroke is far better than stalling inside an interrupt.
- `const fn new()` lets us build the `static` directly (no `lazy_static`
  needed), because the `spin::Mutex` constructor is also `const`.

---

## The consumer side — and the deadlock trap

The main loop drains the queue through this function:

```rust
pub fn next_scancode() -> Option<u8> {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| SCANCODE_QUEUE.lock().pop())
}
```

Why `without_interrupts`? Picture this on a single CPU:

```
   main loop locks the queue to pop ...
        ... keyboard interrupt fires ...
            ... handler tries to lock the SAME queue ...
                ... but the lock is held, so it spins ...
                    ... forever. The main loop can't run to release it.
   => DEADLOCK
```

Disabling interrupts while we hold the lock removes the trap: the keyboard
handler simply can't run until we've popped and released. On a single core,
interrupts-off = guaranteed mutual exclusion.

---

## The whole picture

```
   key press
      |
      v  IRQ1 -> interrupt 33
   keyboard_interrupt_handler (top half)
      - read port 0x60
      - SCANCODE_QUEUE.push(scancode)
      - EOI
      |
      v  (queue)
   next_scancode()  <- called by the main loop (bottom half, Chapter 6)
      - pop with interrupts disabled
      |
      v
   shell.feed_scancode(scancode)  -> decode + line edit + run command
```

---

## What you learned

- The PIC delivers device interrupts; we remap it to 32–47 to dodge exceptions.
- Every hardware handler must send EOI (`notify_end_of_interrupt`).
- The timer doubles as a coarse clock via an `AtomicU64` tick counter.
- Keyboard handling is split: a fast **top half** (ISR → queue) and a slower
  **bottom half** (main loop → shell).
- A fixed ring buffer + **interrupts-off while locked** is the deadlock-free way
  to hand data from an interrupt to the main loop.

**Next:** [Chapter 6 — The shell (REPL + line editing)](06-the-shell-repl.md).
