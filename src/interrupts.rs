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

use lazy_static::lazy_static;
use x86_64::structures::idt::{
    InterruptDescriptorTable,
    InterruptStackFrame,
    PageFaultErrorCode,
};

use crate::{gdt, println};

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
// Exception handlers
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