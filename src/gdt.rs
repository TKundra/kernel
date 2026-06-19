//! # GDT + TSS (a safety net for fatal faults)
//!
//! This is infrastructure you set up once and rarely touch again, but it makes
//! the kernel far more robust.
//!
//! The problem it solves: if the CPU hits a fault (say a stack overflow) and
//! then can't even push the fault information onto the stack, it escalates to a
//! "double fault". If THAT also fails, the CPU triple-faults and the machine
//! reboots — no error message, just a reset.
//!
//! The fix: give the double-fault handler its own, separate, known-good stack.
//! The CPU finds that stack through the Task State Segment (TSS), and the TSS
//! is registered through the Global Descriptor Table (GDT). So we build both
//! here at boot.

use lazy_static::lazy_static;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Which slot in the TSS's "interrupt stack table" holds the double-fault
/// stack. The IDT (in `interrupts.rs`) points the double-fault handler here.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    /// The Task State Segment, holding our dedicated double-fault stack.
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            // A small, statically-allocated stack reserved for double faults.
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            // x86 stacks grow downward, so the CPU wants the *top* (high)
            // address. We take a raw pointer to the static (avoiding a
            // reference to a `static mut`, which the 2024 edition forbids) and
            // add the size to get the end.
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            let stack_end = stack_start + STACK_SIZE as u64;
            stack_end
        };
        tss
    };
}

/// We need to remember the two selectors the GDT hands back so `init` can load
/// them into the CPU's segment registers.
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    /// The Global Descriptor Table: one kernel code segment plus our TSS.
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, tss_selector })
    };
}

/// Load the GDT and point the CPU at our code segment and TSS. Call once, early
/// in boot, before setting up interrupts.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    unsafe {
        // Update the code-segment register to our new descriptor...
        CS::set_reg(GDT.1.code_selector);
        // ...and tell the CPU where the TSS is.
        load_tss(GDT.1.tss_selector);
    }
}
