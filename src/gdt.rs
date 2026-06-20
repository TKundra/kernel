//! # GDT + TSS (CPU safety infrastructure)
//!
//! This module sets up low-level CPU structures that are required for safe
//! exception handling on x86_64 systems.
//!
//! These are not "features" in the usual sense—they are required hardware
//! configuration for correct kernel execution.
//!
//! --------------------------------------------------------------------------
//! PROBLEM THIS SOLVES
//! --------------------------------------------------------------------------
//!
//! When the CPU encounters a fault (e.g. page fault, stack overflow), it
//! normally tries to push error information onto the current stack.
//!
//! But what if the stack itself is broken?
//!
//! Then a **double fault** occurs.
//!
//! If the CPU also cannot handle the double fault (for example, because the
//! stack is invalid again), it triggers a **triple fault**, which immediately
//! resets the machine.
//!
//! This is why a kernel can "mysteriously reboot" with no message.
//!
//! --------------------------------------------------------------------------
//! SOLUTION
//! --------------------------------------------------------------------------
//!
//! We give the CPU a guaranteed safe stack for critical exceptions.
//!
//! - The Task State Segment (TSS) stores a special Interrupt Stack Table (IST)
//! - The Interrupt Descriptor Table (IDT) can tell the CPU:
//!     "for this exception, switch to this safe stack"
//!
//! We use this for the **double fault handler**, ensuring it always has a
//! valid stack, even if everything else is broken.
//!
//! The TSS is activated by registering it in the Global Descriptor Table (GDT).

use lazy_static::lazy_static;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Index of the stack inside the Task State Segment Interrupt Stack Table.
///
/// The IDT will reference this index when handling double faults.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    /// Task State Segment (TSS) containing a dedicated stack for critical faults.
    ///
    /// The IST (Interrupt Stack Table) inside the TSS allows the CPU to switch
    /// stacks automatically during selected exceptions.
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();

        // Allocate a separate stack for handling double faults.
        //
        // IMPORTANT:
        // This stack must NEVER be used for normal execution.
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;

            // Static memory reserved for the double fault stack.
            // This must live for the entire runtime of the kernel.
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            // x86 stacks grow downward, so we provide the *top* of the stack.
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            let stack_end = stack_start + STACK_SIZE as u64;

            stack_end
        };

        tss
    };
}

/// Holds the selectors (indexes) into the GDT.
///
/// These are required later to load CPU segment registers.
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    /// Global Descriptor Table (GDT)
    ///
    /// Contains:
    /// - Kernel code segment (required for execution)
    /// - TSS descriptor (required for interrupt stack switching)
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();

        // Kernel code segment: defines how CPU executes our code
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());

        // TSS descriptor: tells CPU where our Task State Segment is
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(&TSS));

        (gdt, Selectors { code_selector, tss_selector })
    };
}

/// Initializes the GDT and loads the TSS into the CPU.
///
/// MUST be called early during boot, before enabling interrupts.
///
/// After this:
/// - CPU uses our kernel code segment
/// - CPU knows where the TSS is
/// - Interrupts can safely use IST stacks
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    // Load GDT into the CPU's GDTR register
    GDT.0.load();

    unsafe {
        // Set the active code segment (CS register)
        CS::set_reg(GDT.1.code_selector);

        // Load the Task State Segment so CPU can use IST stacks
        load_tss(GDT.1.tss_selector);
    }
}