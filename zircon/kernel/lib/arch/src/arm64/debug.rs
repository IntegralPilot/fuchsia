// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::layout;
use regio::arm64::{SysReg, spec};

/// The number of registers in each of the `DBGB{C,V}R<n>_EL1` and
/// `DBGW{C,V}R<n>_EL1` banks; the count actually implemented is reported by
/// `ID_AA64DFR0_EL1`.
pub const DEBUG_BANK_SIZE: usize = 16;

layout!({
    /// The layout of `DBGBCR<n>_EL1`.
    pub struct BreakpointControlRegister(u64);
    {
        let __ @ 63..24;
        let bt @ 23..20; // Breakpoint type
        let lbn @ 19..16; // Linked breakpoint number
        let ssc @ 15..14; // Security state control
        let hmc @ 13; // Higher mode control
        let __ @ 12..9;
        let bas @ 8..5; // Byte address select
        let __ @ 4..3;
        let pmc @ 2..1; // Privilege mode control
        let e @ 0; // Enable
    }
});

layout!({
    /// The layout of `DBGWCR<n>_EL1`.
    pub struct WatchpointControlRegister(u64);
    {
        let __ @ 63..29;
        let mask @ 28..24; // Address mask
        let __ @ 23..21;
        let wt @ 20; // Watchpoint type
        let lbn @ 19..16; // Linked breakpoint number
        let ssc @ 15..14; // Security state control
        let hmc @ 13; // Higher mode control
        let bas @ 12..5; // Byte address select
        let lsc @ 4..3; // Load/store control
        let pac @ 2..1; // Privilege access control
        let e @ 0; // Enable
    }
});

// Stamps out the consts of an indexed register bank, along with accessors
// dispatching on a runtime index. Register instances are distinct types, so a
// bank cannot be an array.
macro_rules! sysreg_bank {
    (
        $doc:literal, $layout:ty, $read:ident, $write:ident;
        $($n:literal => $name:ident),* $(,)?
    ) => {
        $(
            #[doc = $doc]
            pub const $name: SysReg<spec::$name, $layout> = SysReg::new();
        )*

        /// Reads register `n` of the bank.
        ///
        /// Panics if `n` is not below [`DEBUG_BANK_SIZE`].
        #[cfg(target_arch = "aarch64")]
        pub fn $read(n: usize) -> $layout {
            match n {
                $($n => $name.read(),)*
                _ => panic!("debug register index {n} out of range"),
            }
        }

        /// Writes register `n` of the bank.
        ///
        /// Panics if `n` is not below [`DEBUG_BANK_SIZE`].
        #[cfg(target_arch = "aarch64")]
        pub fn $write(n: usize, value: $layout) {
            match n {
                $($n => $name.write(value),)*
                _ => panic!("debug register index {n} out of range"),
            }
        }
    };
}

sysreg_bank!(
    "[arm/sysreg]/dbgbcrn_el1: DBGBCR<n>_EL1, Debug Breakpoint Control Registers",
    BreakpointControlRegister, read_dbgbcr, write_dbgbcr;
    0 => DBGBCR0_EL1, 1 => DBGBCR1_EL1, 2 => DBGBCR2_EL1, 3 => DBGBCR3_EL1,
    4 => DBGBCR4_EL1, 5 => DBGBCR5_EL1, 6 => DBGBCR6_EL1, 7 => DBGBCR7_EL1,
    8 => DBGBCR8_EL1, 9 => DBGBCR9_EL1, 10 => DBGBCR10_EL1, 11 => DBGBCR11_EL1,
    12 => DBGBCR12_EL1, 13 => DBGBCR13_EL1, 14 => DBGBCR14_EL1, 15 => DBGBCR15_EL1,
);

sysreg_bank!(
    "[arm/sysreg]/dbgbvrn_el1: DBGBVR<n>_EL1, Debug Breakpoint Value Registers",
    u64, read_dbgbvr, write_dbgbvr;
    0 => DBGBVR0_EL1, 1 => DBGBVR1_EL1, 2 => DBGBVR2_EL1, 3 => DBGBVR3_EL1,
    4 => DBGBVR4_EL1, 5 => DBGBVR5_EL1, 6 => DBGBVR6_EL1, 7 => DBGBVR7_EL1,
    8 => DBGBVR8_EL1, 9 => DBGBVR9_EL1, 10 => DBGBVR10_EL1, 11 => DBGBVR11_EL1,
    12 => DBGBVR12_EL1, 13 => DBGBVR13_EL1, 14 => DBGBVR14_EL1, 15 => DBGBVR15_EL1,
);

sysreg_bank!(
    "[arm/sysreg]/dbgwcrn_el1: DBGWCR<n>_EL1, Debug Watchpoint Control Registers",
    WatchpointControlRegister, read_dbgwcr, write_dbgwcr;
    0 => DBGWCR0_EL1, 1 => DBGWCR1_EL1, 2 => DBGWCR2_EL1, 3 => DBGWCR3_EL1,
    4 => DBGWCR4_EL1, 5 => DBGWCR5_EL1, 6 => DBGWCR6_EL1, 7 => DBGWCR7_EL1,
    8 => DBGWCR8_EL1, 9 => DBGWCR9_EL1, 10 => DBGWCR10_EL1, 11 => DBGWCR11_EL1,
    12 => DBGWCR12_EL1, 13 => DBGWCR13_EL1, 14 => DBGWCR14_EL1, 15 => DBGWCR15_EL1,
);

sysreg_bank!(
    "[arm/sysreg]/dbgwvrn_el1: DBGWVR<n>_EL1, Debug Watchpoint Value Registers",
    u64, read_dbgwvr, write_dbgwvr;
    0 => DBGWVR0_EL1, 1 => DBGWVR1_EL1, 2 => DBGWVR2_EL1, 3 => DBGWVR3_EL1,
    4 => DBGWVR4_EL1, 5 => DBGWVR5_EL1, 6 => DBGWVR6_EL1, 7 => DBGWVR7_EL1,
    8 => DBGWVR8_EL1, 9 => DBGWVR9_EL1, 10 => DBGWVR10_EL1, 11 => DBGWVR11_EL1,
    12 => DBGWVR12_EL1, 13 => DBGWVR13_EL1, 14 => DBGWVR14_EL1, 15 => DBGWVR15_EL1,
);
