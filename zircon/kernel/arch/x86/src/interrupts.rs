// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Wrapper type for an interrupt vector. Defined as a wrapped u8, and not an enum, since we do not
/// enumerate every possible value, which would be necessary for a Rust enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X86InterruptVector(pub u8);

impl X86InterruptVector {
    pub const COUNT: usize = 256;
}

pub const X86_INT_DIVIDE_0: X86InterruptVector = X86InterruptVector(0);
pub const X86_INT_DEBUG: X86InterruptVector = X86InterruptVector(1);
pub const X86_INT_NMI: X86InterruptVector = X86InterruptVector(2);
pub const X86_INT_BREAKPOINT: X86InterruptVector = X86InterruptVector(3);
pub const X86_INT_OVERFLOW: X86InterruptVector = X86InterruptVector(4);
pub const X86_INT_BOUND_RANGE: X86InterruptVector = X86InterruptVector(5);
pub const X86_INT_INVALID_OP: X86InterruptVector = X86InterruptVector(6);
pub const X86_INT_DEVICE_NA: X86InterruptVector = X86InterruptVector(7);
pub const X86_INT_DOUBLE_FAULT: X86InterruptVector = X86InterruptVector(8);
pub const X86_INT_INVALID_TSS: X86InterruptVector = X86InterruptVector(0xa);
pub const X86_INT_SEGMENT_NOT_PRESENT: X86InterruptVector = X86InterruptVector(0xb);
pub const X86_INT_STACK_FAULT: X86InterruptVector = X86InterruptVector(0xc);
pub const X86_INT_GP_FAULT: X86InterruptVector = X86InterruptVector(0xd);
pub const X86_INT_PAGE_FAULT: X86InterruptVector = X86InterruptVector(0xe);
pub const X86_INT_RESERVED: X86InterruptVector = X86InterruptVector(0xf);
pub const X86_INT_FPU_FP_ERROR: X86InterruptVector = X86InterruptVector(0x10);
pub const X86_INT_ALIGNMENT_CHECK: X86InterruptVector = X86InterruptVector(0x11);
pub const X86_INT_MACHINE_CHECK: X86InterruptVector = X86InterruptVector(0x12);
pub const X86_INT_SIMD_FP_ERROR: X86InterruptVector = X86InterruptVector(0x13);
pub const X86_INT_VIRT: X86InterruptVector = X86InterruptVector(0x14);
pub const X86_INT_MAX_INTEL_DEFINED: X86InterruptVector = X86InterruptVector(0x1f);

pub const X86_INT_PLATFORM_BASE: X86InterruptVector = X86InterruptVector(0x20);
pub const X86_INT_PLATFORM_MAX: X86InterruptVector = X86InterruptVector(0xef);

pub const X86_INT_LOCAL_APIC_BASE: X86InterruptVector = X86InterruptVector(0xf0);
pub const X86_INT_APIC_SPURIOUS: X86InterruptVector = X86InterruptVector(0xf0);
pub const X86_INT_APIC_TIMER: X86InterruptVector = X86InterruptVector(0xf1);
pub const X86_INT_APIC_ERROR: X86InterruptVector = X86InterruptVector(0xf2);
pub const X86_INT_IPI_GENERIC: X86InterruptVector = X86InterruptVector(0xf3);
pub const X86_INT_IPI_RESCHEDULE: X86InterruptVector = X86InterruptVector(0xf4);
pub const X86_INT_IPI_INTERRUPT: X86InterruptVector = X86InterruptVector(0xf5);
pub const X86_INT_IPI_HALT: X86InterruptVector = X86InterruptVector(0xf6);

pub const X86_INT_MAX: X86InterruptVector = X86InterruptVector(0xff);
