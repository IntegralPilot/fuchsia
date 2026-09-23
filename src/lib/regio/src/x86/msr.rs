// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Accessible, IoHandle, LayoutOver, Register};

/// Example usage:
/// ```
/// use regio::{Register, Rw};
/// use regio::x64::MsrIo;
///
/// const IA32_TIME_STAMP_COUNTER: Msr<0x10, u64, Rw> = unsafe { Msr::new() };
/// ```
pub type Msr<const ID: u32, Layout, Access> = Register<Layout, Access, MsrIo<ID>>;

impl<const ID: u32, Layout, Access> Msr<ID, Layout, Access>
where
    Layout: LayoutOver<u64>,
    Access: Accessible,
{
    /// Constructs an x86-64 MSR instance.
    pub const fn new() -> Self {
        // Safety: There is nothing unsafe about MsrIo construction.
        unsafe { Self::from_io(MsrIo {}) }
    }
}

/// A simple I/O backend for reading from and writing to MSRs.
pub struct MsrIo<const ID: u32> {}

impl<const ID: u32> IoHandle for MsrIo<ID> {
    type Base = u64;
}

#[cfg(target_arch = "x86_64")]
mod x86_64_only {
    use super::*;
    use crate::{ReadHandle, WriteHandle};

    use core::arch::asm;

    impl<const ID: u32> ReadHandle for MsrIo<ID> {
        #[inline]
        unsafe fn read_raw(&self) -> u64 {
            let hi: u32;
            let lo: u32;
            unsafe {
                asm!(
                    "rdmsr",
                    in("ecx") ID,
                    out("eax") lo,
                    out("edx") hi,
                    options(nomem, nostack, preserves_flags),
                )
            };
            u64::from(hi) << 32 | u64::from(lo)
        }
    }

    impl<const ID: u32> WriteHandle for MsrIo<ID> {
        #[inline]
        unsafe fn write_raw(&self, value: u64) {
            let hi = (value >> 32) as u32;
            let lo = value as u32;
            unsafe {
                asm!(
                    "wrmsr",
                    in("ecx") ID,
                    in("eax") lo,
                    in("edx") hi,
                    options(nomem, nostack, preserves_flags),
                )
            }
        }
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use crate::RwSafe;

    // MSRs are privileged instructions, but these abstractions are fine to
    // compile in under any x86-64 environment so long as we don't actually
    // perform the MSR access at runtime.
    #[test]
    fn test_msr_compilation() {
        #[allow(unused)]
        const IA32_TIME_STAMP_COUNTER: Msr<0x10, u64, RwSafe> = Msr::new();
    }
}
