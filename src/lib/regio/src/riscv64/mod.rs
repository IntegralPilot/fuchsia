// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod encoding;

use core::marker::PhantomData;

use crate::{Accessible, IoHandle, LayoutOver, Register, Ro, RwSafe, RwUnsafe, WoSafe, WoUnsafe};

/// Models a riscv64 CSR with a given layout.
///
/// It takes a CSR encoding as a generic parameter, all of which have been
/// stamped out in the `regio::riscv64::encoding` submodule with names equal to
/// their official mnemonics.
///
/// Example usage:
/// ```rust
/// use regio::riscv64::{Csr, encoding};
///
/// const TIME: Csr<encoding::time, u64> = Csr::new();
///
/// println!("TIME: {:#x}", TIME.read());
/// ```
pub type Csr<Encoding, Layout> =
    Register<Layout, <Encoding as ControlAndStatusRegisterEncoding>::Access, CsrIo<Encoding>>;

/// Marker for a riscv64 CSR, expressing its encoding and access permissions.
pub trait ControlAndStatusRegisterEncoding {
    /// The register's 12-bit encoding value.
    const VALUE: u16;

    /// The register's access permissions.
    type Access: Accessible;

    /// Whether the encoding is valid.
    const VALID: ();
}

/// Tag for a riscv64 CSR, expressing its encoding and access permissions.
pub struct CsrEncoding<const ENCODING: u16, Access: Accessible>(PhantomData<Access>);

macro_rules! impl_csr_encoding {
    ($access:ty, $validate:block) => {
        impl<const ENCODING: u16> ControlAndStatusRegisterEncoding
            for CsrEncoding<ENCODING, $access>
        {
            const VALUE: u16 = ENCODING;
            type Access = $access;

            const VALID: () = {
                assert!(Self::VALUE <= 0xfff, "CSR encoding must be 12-bit");
                $validate
            };
        }
    };
}

impl_csr_encoding!(Ro, {
    assert!(
        (ENCODING >> 10) & 0b11_u16 == 0b11_u16,
        "Associated access type is read-only but the encoding is not"
    );
});
impl_csr_encoding!(RwSafe, {});
impl_csr_encoding!(RwUnsafe, {});
impl_csr_encoding!(WoSafe, {
    assert!(false, "All CSRs are readable");
});
impl_csr_encoding!(WoUnsafe, {
    assert!(false, "All CSRs are readable");
});

impl<Encoding, Layout> Csr<Encoding, Layout>
where
    Encoding: ControlAndStatusRegisterEncoding,
    Layout: LayoutOver<u64>,
{
    /// Constructs a new CSR instance.
    pub const fn new() -> Self {
        // Safety: There is nothing unsound about CsrIo construction.
        unsafe { Self::from_io(CsrIo::new()) }
    }
}

/// An I/O backend for riscv64 CSRs.
pub struct CsrIo<Encoding: ControlAndStatusRegisterEncoding>(PhantomData<Encoding>);

impl<Encoding: ControlAndStatusRegisterEncoding> CsrIo<Encoding> {
    pub const fn new() -> Self {
        // Associated constants are evaluated lazily, so force an evaluation
        // now.
        let _ = Encoding::VALID;
        Self(PhantomData)
    }
}

impl<Encoding: ControlAndStatusRegisterEncoding> IoHandle for CsrIo<Encoding> {
    type Base = u64;
}

#[cfg(target_arch = "riscv64")]
mod riscv64_only {
    use core::arch::asm;

    use super::*;
    use crate::{AtomicIoHandle, ReadHandle, Readable, Writable, WriteHandle};

    impl<Encoding: ControlAndStatusRegisterEncoding> ReadHandle for CsrIo<Encoding>
    where
        Encoding::Access: Readable,
    {
        #[inline]
        unsafe fn read_raw(&self) -> u64 {
            let value: u64;
            unsafe {
                asm!(
                    "csrr {value}, {csr}",
                    value = out(reg) value,
                    csr = const Encoding::VALUE,
                    options(nomem, nostack, preserves_flags),
                )
            }
            value
        }
    }

    impl<Encoding: ControlAndStatusRegisterEncoding> WriteHandle for CsrIo<Encoding>
    where
        Encoding::Access: Writable,
    {
        #[inline]
        unsafe fn write_raw(&self, value: u64) {
            unsafe {
                asm!(
                    "csrw {csr}, {value}",
                    value = in(reg) value,
                    csr = const Encoding::VALUE,
                    // TODO(https://fxbug.dev/525077555): Revisit using nomem here.
                    options(nostack, preserves_flags),
                )
            }
        }
    }

    impl<Encoding: ControlAndStatusRegisterEncoding> AtomicIoHandle for CsrIo<Encoding>
    where
        Encoding::Access: Readable + Writable,
    {
        #[inline]
        unsafe fn atomic_swap_raw(&self, value: u64) -> u64 {
            let previous: u64;
            unsafe {
                asm!(
                    "csrrw {previous}, {csr}, {value}",
                    previous = out(reg) previous,
                    value = in(reg) value,
                    csr = const Encoding::VALUE,
                    // TODO(https://fxbug.dev/525077555): Revisit using nomem here.
                    options(nostack, preserves_flags),
                )
            };
            previous
        }

        #[inline]
        unsafe fn atomic_set_bits_raw(&self, bits: u64) -> u64 {
            let previous: u64;
            unsafe {
                asm!(
                    "csrrs {previous}, {csr}, {bits}",
                    previous = out(reg) previous,
                    bits = in(reg) bits,
                    csr = const Encoding::VALUE,
                    // TODO(https://fxbug.dev/525077555): Revisit using nomem here.
                    options(nostack, preserves_flags),
                )
            };
            previous
        }

        #[inline]
        unsafe fn atomic_clear_bits_raw(&self, bits: u64) -> u64 {
            let previous: u64;
            unsafe {
                asm!(
                    "csrrc {previous}, {csr}, {bits}",
                    previous = out(reg) previous,
                    bits = in(reg) bits,
                    csr = const Encoding::VALUE,
                    // TODO(https://fxbug.dev/525077555): Revisit using nomem here.
                    options(nostack, preserves_flags),
                )
            };
            previous
        }
    }
}

#[cfg(all(test, target_arch = "riscv64"))]
mod tests {
    use super::*;

    #[test]
    fn csrs() {
        const TIME: Csr<encoding::time, u64> = Csr::new();

        println!("Current time: {:#x}", TIME.read());
    }
}
