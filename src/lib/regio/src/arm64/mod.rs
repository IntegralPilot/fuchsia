// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod spec;

use crate::{Accessible, IoHandle, LayoutOver, Register};

use core::marker::PhantomData;

/// Marker for an arm64 system register, expressing its encoding and access
/// permissons.
pub trait SystemRegisterSpec {
    /// The register encoding's op0 parameter.
    const OP0: u8;

    /// The register encoding's op1 parameter.
    const OP1: u8;

    /// The register encoding's CRn parameter.
    const CRN: u8;

    /// The register encoding's CRm parameter.
    const CRM: u8;

    /// The register encoding's op2 parameter.
    const OP2: u8;

    /// The register's access permissions.
    type Access: Accessible;

    /// Whether the encoding is valid.
    const VALID: () = {
        assert!(Self::OP0 == 2 || Self::OP0 == 3, "op0 must be 2 or 3");
        assert!(Self::OP1 <= 7, "op1 must be in between 0 and 7");
        assert!(Self::CRN <= 15, "CRn must be in between 0 and 15");
        assert!(Self::CRM <= 15, "CRm must be in between 0 and 15");
        assert!(Self::OP2 <= 7, "op2 must be in between 0 and 7");
    };
}

/// Tag for an arm64 system register, expressing its encoding and access
/// permissons.
pub struct SysRegSpec<
    const OP0: u8,
    const OP1: u8,
    const CRN: u8,
    const CRM: u8,
    const OP2: u8,
    Access: Accessible,
>(PhantomData<Access>);

impl<const OP0: u8, const OP1: u8, const CRN: u8, const CRM: u8, const OP2: u8, Access: Accessible>
    SystemRegisterSpec for SysRegSpec<OP0, OP1, CRN, CRM, OP2, Access>
{
    const OP0: u8 = OP0;
    const OP1: u8 = OP1;
    const CRN: u8 = CRN;
    const CRM: u8 = CRM;
    const OP2: u8 = OP2;
    type Access = Access;
}

/// Models an arm64 system register with a given layout.
///
/// It takes a system register 'spec' as a generic parameter, all of which have
/// been stamped out in the `regio::arm64::spec` submodule with names equal to
/// their official mnemonics.
///
/// Example usage:
/// ```rust
/// use regio::arm64::{SysReg, spec};
///
/// const TPIDR_EL0: SysReg<spec::TPIDR_EL0, u64> = SysReg::new();
///
///println!("TPIDR_EL0: {:#x}", TPIDR_EL0.read().get());
/// ```
pub type SysReg<Spec, Layout> =
    Register<Layout, <Spec as SystemRegisterSpec>::Access, SysRegIo<Spec>>;

impl<Spec, Layout> SysReg<Spec, Layout>
where
    Spec: SystemRegisterSpec,
    Layout: LayoutOver<u64>,
{
    /// Constructs a new system register instance.
    pub const fn new() -> Self {
        // Safety: There is nothing unsafe about SysRegIo construction.
        unsafe { Self::from_io(SysRegIo::new()) }
    }
}

/// An I/O backend for arm64 system registers.
pub struct SysRegIo<Spec: SystemRegisterSpec>(PhantomData<Spec>);

impl<Spec: SystemRegisterSpec> SysRegIo<Spec> {
    pub const fn new() -> Self {
        // Associated constants are evaluated lazily, so force an evaluation
        // now.
        let _ = Spec::VALID;
        Self(PhantomData)
    }
}

impl<Spec: SystemRegisterSpec> IoHandle for SysRegIo<Spec> {
    type Base = u64;
}

#[cfg(target_arch = "aarch64")]
mod arm64_only {
    use core::arch::asm;

    use super::*;
    use crate::{ReadHandle, Readable, Writable, WriteHandle};

    impl<Spec: SystemRegisterSpec> ReadHandle for SysRegIo<Spec>
    where
        Spec::Access: Readable,
    {
        #[inline]
        unsafe fn read_raw(&self) -> u64 {
            let value: u64;
            unsafe {
                asm!(
                    "mrs {value}, S{op0}_{op1}_C{crn}_C{crm}_{op2}",
                    value = out(reg) value,
                    op0 = const Spec::OP0,
                    op1 = const Spec::OP1,
                    crn = const Spec::CRN,
                    crm = const Spec::CRM,
                    op2 = const Spec::OP2,
                    options(nomem, nostack, preserves_flags),
                )
            }
            value
        }
    }

    impl<Spec: SystemRegisterSpec> WriteHandle for SysRegIo<Spec>
    where
        Spec::Access: Writable,
    {
        #[inline]
        unsafe fn write_raw(&self, value: u64) {
            unsafe {
                asm!(
                    "msr S{op0}_{op1}_C{crn}_C{crm}_{op2}, {value}",
                    value = in(reg) value,
                    op0 = const Spec::OP0,
                    op1 = const Spec::OP1,
                    crn = const Spec::CRN,
                    crm = const Spec::CRM,
                    op2 = const Spec::OP2,
                    // TODO(https://fxbug.dev/525077555): Revisit using nomem here.
                    options(nostack, preserves_flags),
                )
            }
        }
    }
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;

    #[test]
    fn sysregs() {
        const TPIDR_EL0: SysReg<spec::TPIDR_EL0, u64> = SysReg::new();

        println!("TPIDR_EL0: {:#x}", TPIDR_EL0.read());
    }
}
