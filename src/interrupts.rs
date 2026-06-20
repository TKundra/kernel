//! # Interrupts: IDT, exceptions, and hardware interrupts
//!
//! Interrupts are events that temporarily pause normal CPU execution so a
//! handler can run, after which execution resumes.
//!
//! There are two main categories:
//!
//! ## 1. CPU exceptions (synchronous)
//! Triggered directly by the CPU when something goes wrong in instruction
//! execution.
//!
//! Examples:
//! - breakpoint (`int3`)
//! - page fault (#PF)
//! - double fault (#DF)
//!
//! ## 2. Hardware interrupts (asynchronous)
//! Triggered by external devices via the Programmable Interrupt Controller (PIC),
//! e.g. keyboard, timer, mouse.
//!
//! --------------------------------------------------------------------------
//! INTERRUPT DESCRIPTOR TABLE (IDT)
//! --------------------------------------------------------------------------
//!
//! The CPU uses the Interrupt Descriptor Table (IDT) to decide which function
//! to call for each interrupt/exception vector.
//!
//! Each entry maps:
//!     interrupt number → handler function
//!
//! We construct and load this table once during boot.
//!
//! --------------------------------------------------------------------------
//! KEYBOARD DESIGN NOTE
//! --------------------------------------------------------------------------
//!
//! We follow a "top half / bottom half" design:
//!
//! - Top half (interrupt handler):
//!     Runs in interrupt context, must be extremely fast.
//!     Only reads the keyboard scancode and stores it in a buffer.
//!
//! - Bottom half (main loop):
//!     Performs slow work like:
//!     - decoding scancodes into characters
//!     - command parsing
//!     - terminal logic
//!
//! WHY:
//! Doing heavy work inside an interrupt handler can:
//! - block other interrupts
//! - increase latency
//! - cause deadlocks (especially if locks are involved)

use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{
    InterruptDescriptorTable,
    InterruptStackFrame,
    PageFaultErrorCode,
};

use crate::{gdt, println};

/// The two PICs are remapped to interrupt numbers 32..47, because 0..31 are
/// reserved by the CPU for exceptions. Timer = 32, keyboard = 33.
pub const PIC_1_OFFSET: u8 = 32;               // 32 = 0x20
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8; // 40 = 0x28

/// Named interrupt numbers for the hardware interrupts we handle.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,  // 32
    Keyboard,              // 33
}

impl InterruptIndex {
    fn as_u8(self) -> u8 { self as u8 }
    fn as_usize(self) -> usize { self as usize }
}

/// The pair of chained PICs. Marked `unsafe` to construct because giving wrong
/// offsets could misroute interrupts. `notify_end_of_interrupt` must be called
/// at the end of every hardware-interrupt handler or no further interrupts of
/// that kind will arrive.
pub static PICS: Mutex<ChainedPics> = Mutex::new(
    unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) }
);

lazy_static! {
    /// The Interrupt Descriptor Table (IDT)
    ///
    /// This is loaded into the CPU once at boot and defines how all interrupts
    /// and exceptions are handled.
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // ------------------------------------------------------------------
        // CPU exception handlers
        // ------------------------------------------------------------------

        // Breakpoint exception (int3) — mainly used for debugging
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        // Page fault (#PF) — memory access violation
        idt.page_fault.set_handler_fn(page_fault_handler);

        // Double fault (#DF) — critical failure during exception handling
        //
        // We assign it a dedicated stack via the TSS (see gdt.rs).
        // This prevents stack overflow cases from escalating into a triple fault
        // (which would reset the CPU).
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }

        // Hardware interrupt handlers for the PIC-mapped IRQs.
        // add entries for the interrupts we want to handle (timer, keyboard)
        // these handlers will be called when the corresponding hardware interrupts fire (Vector 32 for timer, 33 for keyboard)
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);

        idt
    };
}

/// Load the IDT into the CPU.
///
/// Must be called during early boot, before interrupts are enabled.
pub fn init_idt() {
    IDT.load();
}

// -------------------------------------------------------------------------
// CPU Exception handlers
// -------------------------------------------------------------------------

/// Breakpoint exception (`int3`)
///
/// This is recoverable. Execution continues after the handler returns.
extern "x86-interrupt" fn breakpoint_handler(
    stack_frame: InterruptStackFrame,
) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

/// Double fault (#DF)
///
/// This is a fatal exception caused when handling another exception fails.
///
/// With our TSS-based IST stack, we *usually* reach this handler safely.
/// If this handler runs, we treat it as unrecoverable and panic.
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

/// Page fault (#PF)
///
/// Triggered when the CPU cannot translate a virtual memory address:
/// - page not mapped
/// - invalid permissions
/// - protection violation
///
/// The faulting address is stored in CR2 by the CPU.
///
/// Without this handler, a page fault would escalate to a double fault,
/// and possibly a system reset (triple fault).
///
/// NOTE:
/// This handler cannot safely “continue execution” unless the fault is
/// explicitly handled (e.g. demand paging in a full OS).
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    println!("EXCEPTION: PAGE FAULT");
    println!("  faulting address: {:?}", Cr2::read());
    println!("  error code: {:?}", error_code);

    panic!("unhandled page fault\n{:#?}", stack_frame);
}

// -------------------------------------------------------------------------
// Hardware interrupt handlers
// -------------------------------------------------------------------------

/// Counts how many timer interrupts have fired since boot.
///
/// The legacy PIT (Programmable Interval Timer) generates IRQ0 at ~18.2 Hz
/// by default (unless reprogrammed).
///
/// This gives us a very rough notion of time, used for:
/// - uptime
/// - simple scheduling experiments
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Timer frequency × 10 (18.2 Hz → 182).
///
/// We store it scaled by 10 so we can compute time using integer math
/// instead of floating point (which is avoided in kernels).
pub const TIMER_HZ_X10: u64 = 182;

/// Returns number of timer ticks since boot.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// ------------------------------------------------------------------------
/// TIMER INTERRUPT (IRQ0)
/// ------------------------------------------------------------------------
///
/// Fired periodically by the PIT (~18.2 Hz).
///
/// Responsibilities:
/// 1. Increment global tick counter
/// 2. Acknowledge interrupt to the PIC
///
/// IMPORTANT:
/// If we do NOT send EOI (End Of Interrupt), the PIC will stop delivering
/// further interrupts on this line.
extern "x86-interrupt" fn timer_interrupt_handler(
    _stack_frame: InterruptStackFrame,
) {
    TICKS.fetch_add(1, Ordering::Relaxed);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

/// ------------------------------------------------------------------------
/// KEYBOARD INTERRUPT (IRQ1)
/// ------------------------------------------------------------------------
///
/// This is the "top half" of keyboard handling.
///
/// Goal: do the absolute minimum work:
/// - read scancode from controller
/// - push into queue
/// - acknowledge interrupt
///
/// Everything else (decoding, printing, editing) happens in the main loop.
///
/// WHY THIS IS IMPORTANT:
/// Interrupt handlers run with interrupts disabled → they must be fast or
/// they will block the entire system.
extern "x86-interrupt" fn keyboard_interrupt_handler(
    _stack_frame: InterruptStackFrame,
) {
    use x86_64::instructions::port::Port;

    // PS/2 keyboard data port
    let mut port = Port::new(0x60);

    // Must read this byte, otherwise the keyboard will stop generating IRQs
    let scancode: u8 = unsafe { port.read() };

    SCANCODE_QUEUE.lock().push(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

// ------------------------------------------------------------------------
// Scancode queue (ISR → main loop communication)
// ------------------------------------------------------------------------

const QUEUE_CAPACITY: usize = 256;

/// Fixed-size ring buffer for keyboard input.
///
/// This is the bridge between:
/// - interrupt context (producer)
/// - main loop (consumer)
///
/// No heap allocation is used; everything is static memory.
struct ScancodeQueue {
    buffer: [u8; QUEUE_CAPACITY],
    head: usize,   // next item to read
    tail: usize,   // next slot to write
    count: usize,  // number of elements currently stored
}

impl ScancodeQueue {
    const fn new() -> Self {
        Self {
            buffer: [0; QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Push a scancode into the queue.
    ///
    /// If full, we drop the input instead of blocking inside an interrupt.
    /// This is intentional: ISRs must never block or spin.
    fn push(&mut self, value: u8) {
        if self.count == QUEUE_CAPACITY {
            return;
        }

        self.buffer[self.tail] = value;
        self.tail = (self.tail + 1) % QUEUE_CAPACITY;
        self.count += 1;
    }

    /// Pop the oldest scancode.
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

/// Global keyboard buffer protected by a spinlock.
///
/// Used by both:
/// - keyboard interrupt handler (push)
/// - main loop (pop)
static SCANCODE_QUEUE: Mutex<ScancodeQueue> = Mutex::new(ScancodeQueue::new());

/// Retrieve the next scancode from the queue.
///
/// We disable interrupts while locking to avoid deadlock:
///
/// Scenario without this protection:
/// - main holds lock
/// - keyboard interrupt fires
/// - ISR tries to lock → deadlock
///
/// Disabling interrupts prevents ISR from running mid-critical section.
pub fn next_scancode() -> Option<u8> {
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        SCANCODE_QUEUE.lock().pop()
    })
}