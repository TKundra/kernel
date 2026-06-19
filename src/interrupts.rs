//! # Interrupts: the IDT, the PIC, and the keyboard ISR
//!
//! An interrupt is the CPU stopping what it's doing to run a handler, then
//! resuming. There are two kinds we care about:
//!
//!   * CPU exceptions (e.g. breakpoint, double fault) — raised by the CPU.
//!   * Hardware interrupts (timer, keyboard) — raised by devices via the PIC.
//!
//! The CPU finds the right handler through the Interrupt Descriptor Table
//! (IDT): a table mapping each interrupt number to a function. We build that
//! table here.
//!
//! For the keyboard we follow the standard "top half / bottom half" split:
//! the interrupt handler (top half) does the bare minimum — read the scancode
//! and stash it in a queue — and the slow work (decoding, line editing,
//! running commands) happens later in the main loop (bottom half). Doing heavy
//! work inside an interrupt handler is bad practice and risks deadlocks.

use core::sync::atomic::{AtomicU64, Ordering};

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{gdt, println};

/// The two PICs are remapped to interrupt numbers 32..47, because 0..31 are
/// reserved by the CPU for exceptions. Timer = 32, keyboard = 33.
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// Named interrupt numbers for the hardware interrupts we handle.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard, // = 33 (the next value after Timer)
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

/// The pair of chained PICs. Marked `unsafe` to construct because giving wrong
/// offsets could misroute interrupts. `notify_end_of_interrupt` must be called
/// at the end of every hardware-interrupt handler or no further interrupts of
/// that kind will arrive.
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    /// The interrupt table. Built once, then loaded into the CPU by `init`.
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // CPU exceptions:
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        unsafe {
            // Route the double-fault handler to its dedicated stack (set up in
            // gdt.rs) so a stack overflow can't turn into a triple fault.
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }

        // Hardware interrupts:
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);

        idt
    };
}

/// Load the IDT into the CPU. Call once at boot.
pub fn init_idt() {
    IDT.load();
}

// ---- CPU exception handlers --------------------------------------------

/// `int3` breakpoint. Recoverable: we print and return, execution continues.
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

/// Double fault. Not recoverable, so the handler must diverge (`-> !`). We turn
/// it into a kernel panic with the saved CPU state.
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

/// Page fault (#PF). Raised when code touches memory that isn't mapped (or
/// isn't allowed). The faulting address is left in the CR2 register by the CPU.
/// Our `peek` command can hit this if you read an unmapped address — without
/// this handler that would escalate to a triple fault and reboot the machine.
/// We can't safely continue (the instruction would just fault again), so we
/// report the details and panic.
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

// ---- Hardware interrupt handlers ---------------------------------------

/// Counts how many timer interrupts have fired since boot. The PIC's timer
/// (the legacy PIT) ticks at ~18.2 Hz by default, which we never reprogram, so
/// this doubles as a coarse clock for the `uptime` command.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Approximate timer frequency times 10 (18.2 Hz -> 182). Stored x10 so we can
/// do the tick->seconds conversion with integer math (no floating point).
pub const TIMER_HZ_X10: u64 = 182;

/// Number of timer ticks since boot.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Timer tick (IRQ0). The PIC fires this ~18 times a second. We bump the tick
/// counter and must acknowledge the interrupt (or it would block all later
/// interrupts).
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

/// Keyboard (IRQ1) — the "top half". Read the one scancode the controller has
/// for us, push it into the queue, acknowledge the interrupt, and return fast.
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Port 0x60 is the PS/2 data port. We MUST read it, or the controller
    // won't send the next interrupt.
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    SCANCODE_QUEUE.lock().push(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

// ---- Scancode queue (the hand-off between top and bottom half) ----------

const QUEUE_CAPACITY: usize = 256;

/// A fixed-size ring buffer of scancodes. No heap allocation — everything lives
/// in this static. Written by the keyboard ISR, drained by the main loop.
struct ScancodeQueue {
    buffer: [u8; QUEUE_CAPACITY],
    head: usize,  // index to read next
    tail: usize,  // index to write next
    count: usize, // how many bytes are currently stored
}

impl ScancodeQueue {
    const fn new() -> Self {
        ScancodeQueue {
            buffer: [0; QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Add a scancode. If the queue is full we drop the byte — better to lose a
    /// keystroke than to block inside an interrupt handler.
    fn push(&mut self, value: u8) {
        if self.count == QUEUE_CAPACITY {
            return;
        }
        self.buffer[self.tail] = value;
        self.tail = (self.tail + 1) % QUEUE_CAPACITY;
        self.count += 1;
    }

    /// Remove and return the oldest scancode, or `None` if empty.
    fn pop(&mut self) -> Option<u8> {
        if self.count == 0 {
            return None;
        }
        let value = self.buffer[self.head];
        self.head = (self.head + 1) % QUEUE_CAPACITY;
        self.count -= 1;
        Some(value)
    }
}

/// The queue is a plain `static` (the constructor is `const`), guarded by a
/// spinlock.
static SCANCODE_QUEUE: Mutex<ScancodeQueue> = Mutex::new(ScancodeQueue::new());

/// Pop the next scancode for the main loop to process.
///
/// We disable interrupts while holding the lock. This is the key to avoiding
/// deadlock on a single CPU: the keyboard ISR also locks this queue, so if an
/// interrupt fired while we held the lock, it would spin forever. With
/// interrupts off, the ISR can't run until we're done.
pub fn next_scancode() -> Option<u8> {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| SCANCODE_QUEUE.lock().pop())
}
