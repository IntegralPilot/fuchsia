// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::dev_interrupt::{InterruptPolarity, InterruptTriggerMode};

pub const NUM_ISA_IRQS: usize = 16;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ApicInterruptDeliveryMode {
    Fixed = 0,
    LowestPri = 1,
    SMI = 2,
    NMI = 4,
    Init = 5,
    Startup = 6,
    ExtInt = 7,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApicInterruptDstMode {
    Physical = 0,
    Logical = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GsiRange {
    pub start: u32,
    pub end: u32,
}

#[repr(C)]
#[derive(Debug, Clone)]
/// Information about the system IO APICs
pub struct IoApicDescriptor {
    pub apic_id: u8,
    /// virtual IRQ base for ACPI
    pub global_irq_base: u32,
    /// Physical address of the base of this IOAPIC's MMIO
    pub paddr: usize,
}

#[repr(C)]
#[derive(Debug, Clone)]
/// Information describing an ISA override.  An override can change the
/// global IRQ number and/or change bus signaling characteristics
/// for the specified ISA IRQ.
pub struct IoApicIsaOverride {
    pub isa_irq: u8,
    pub remapped: bool,
    pub tm: InterruptTriggerMode,
    pub pol: InterruptPolarity,
    pub global_irq: u32,
}

pub const IO_APIC_IRQ_MASK: bool = true;
pub const IO_APIC_IRQ_UNMASK: bool = false;

const _: () = {
    assert!(core::mem::size_of::<IoApicDescriptor>() == 16);
    assert!(core::mem::align_of::<IoApicDescriptor>() == 8);
    assert!(core::mem::offset_of!(IoApicDescriptor, apic_id) == 0);
    assert!(core::mem::offset_of!(IoApicDescriptor, global_irq_base) == 4);
    assert!(core::mem::offset_of!(IoApicDescriptor, paddr) == 8);

    assert!(core::mem::size_of::<IoApicIsaOverride>() == 16);
    assert!(core::mem::align_of::<IoApicIsaOverride>() == 4);
    assert!(core::mem::offset_of!(IoApicIsaOverride, isa_irq) == 0);
    assert!(core::mem::offset_of!(IoApicIsaOverride, remapped) == 1);
    assert!(core::mem::offset_of!(IoApicIsaOverride, tm) == 4);
    assert!(core::mem::offset_of!(IoApicIsaOverride, pol) == 8);
    assert!(core::mem::offset_of!(IoApicIsaOverride, global_irq) == 12);

    assert!(core::mem::size_of::<GsiRange>() == 8);
    assert!(core::mem::align_of::<GsiRange>() == 4);
    assert!(core::mem::offset_of!(GsiRange, start) == 0);
    assert!(core::mem::offset_of!(GsiRange, end) == 4);
};
