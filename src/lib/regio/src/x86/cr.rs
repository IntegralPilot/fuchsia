// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Accessible, IoHandle, LayoutOver, Register, Ro, RwSafe, RwUnsafe};

/// Trait implemented by supported control registers, fixing their access permissions.
pub trait ControlRegister {
    type Access: Accessible;
}

impl ControlRegister for CrIo<0> {
    // Can enable/disable paging (PG) and caching (CD, NW).
    type Access = RwUnsafe;
}

impl ControlRegister for CrIo<2> {
    type Access = Ro;
}

impl ControlRegister for CrIo<3> {
    // Can switch the active page table root and PCID.
    type Access = RwUnsafe;
}

impl ControlRegister for CrIo<4> {
    // Can alter virtual memory mode (PAE, LA57, PCIDE) and execution protection (SMEP, SMAP, PKE).
    type Access = RwUnsafe;
}

impl ControlRegister for CrIo<8> {
    type Access = RwSafe;
}

impl ControlRegister for XcrIo<0> {
    type Access = RwSafe;
}

/// Example usage:
/// ```
/// use regio::x86::Cr;
///
/// const CR0: Cr<0, u64> = Cr::new();
/// ```
pub type Cr<const N: u32, Layout> = Register<Layout, <CrIo<N> as ControlRegister>::Access, CrIo<N>>;

impl<const N: u32, Layout> Cr<N, Layout>
where
    CrIo<N>: ControlRegister,
    Layout: LayoutOver<u64>,
{
    /// Constructs an x86-64 control register instance.
    pub const fn new() -> Self {
        // Safety: There is nothing unsafe about CrIo construction.
        unsafe { Self::from_io(CrIo {}) }
    }
}

/// Example usage:
/// ```
/// use regio::x86::Xcr;
///
/// const XCR0: Xcr<0, u64> = Xcr::new();
/// ```
pub type Xcr<const N: u32, Layout> =
    Register<Layout, <XcrIo<N> as ControlRegister>::Access, XcrIo<N>>;

impl<const N: u32, Layout> Xcr<N, Layout>
where
    XcrIo<N>: ControlRegister,
    Layout: LayoutOver<u64>,
{
    /// Constructs an x86-64 control register instance.
    pub const fn new() -> Self {
        // Safety: There is nothing unsafe about XcrIo construction.
        unsafe { Self::from_io(XcrIo {}) }
    }
}

/// A simple I/O backend for reading from and writing to control registers.
pub struct CrIo<const N: u32> {}

impl<const N: u32> IoHandle for CrIo<N> {
    type Base = u64;
}

/// A simple I/O backend for reading from and writing to extended control registers.
pub struct XcrIo<const N: u32> {}

impl<const N: u32> IoHandle for XcrIo<N> {
    type Base = u64;
}

#[cfg(target_arch = "x86_64")]
mod x86_64_only {
    use core::arch::asm;

    use super::*;
    use crate::{ReadHandle, WriteHandle};

    macro_rules! impl_cr_io {
        ($n:literal, $cr_str:literal) => {
            impl ReadHandle for CrIo<$n> {
                #[inline]
                unsafe fn read_raw(&self) -> u64 {
                    let value: u64;
                    unsafe {
                        asm!(
                            concat!("mov {value}, ", $cr_str),
                            value = out(reg) value,
                            options(nomem, nostack, preserves_flags),
                        );
                    }
                    value
                }
            }

            impl WriteHandle for CrIo<$n> {
                #[inline]
                unsafe fn write_raw(&self, value: u64) {
                    unsafe {
                        asm!(
                            concat!("mov ", $cr_str, ", {value}"),
                            value = in(reg) value,
                            // TODO(https://fxbug.dev/525077555): Revisit using nomem here.
                            options(nostack, preserves_flags),
                        );
                    }
                }
            }
        };
    }

    impl_cr_io!(0, "cr0");
    impl_cr_io!(2, "cr2");
    impl_cr_io!(3, "cr3");
    impl_cr_io!(4, "cr4");
    impl_cr_io!(8, "cr8");
}

#[cfg(all(target_arch = "x86_64", feature = "xsave"))]
mod x86_64_xsave_only {
    use core::arch::x86::{_xgetbv, _xsetbv};

    use super::*;
    use crate::{ReadHandle, WriteHandle};

    macro_rules! impl_xcr_io {
        ($n:literal) => {
            impl ReadHandle for XcrIo<$n> {
                #[inline]
                unsafe fn read_raw(&self) -> u64 {
                    unsafe { _xgetbv($n) }
                }
            }

            impl WriteHandle for XcrIo<$n> {
                #[inline]
                unsafe fn write_raw(&self, value: u64) {
                    unsafe { _xsetbv($n, value) }
                }
            }
        };
    }
    impl_xcr_io!(0);
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn test_cr_compilation() {
        #[allow(unused)]
        {
            const CR0: Cr<0, u64> = Cr::new();
            const CR2: Cr<2, u64> = Cr::new();
            const CR3: Cr<3, u64> = Cr::new();
            const CR4: Cr<4, u64> = Cr::new();
            const CR8: Cr<8, u64> = Cr::new();
        }
    }
}
