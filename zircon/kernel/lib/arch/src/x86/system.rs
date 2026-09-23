// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::layout;
use regio::x86::Cr;

/// [intel/vol3]: 2.5 Control Registers: CR0
pub const CR0: Cr<0, ControlRegister0> = Cr::new();

layout!({
    /// The layout of [`CR0`].
    pub struct ControlRegister0(u64);
    {
        let __ @ 63..32 = 0;
        let pg @ 31; // Paging enabled
        let cd @ 30; // Cache disabled
        let nw @ 29; // Not write-through
        // Bits [28:19] are reserved.
        let __ @ 28..19;
        let am @ 18; // Alignment mask (support alignment checking)
        // Bit 17 is reserved.
        let __ @ 17;
        let wp @ 16; // Write protect (prevent supervisor writing to RO pages)
        // Bits [15:6] are reserved.
        let __ @ 15..6;
        let ne @ 5; // Numeric error (control FPU exceptions)
        let et @ 4; // Extension type (reserved on modern CPUs, always 1)
        let ts @ 3; // Task switched (trap on FPU/MMX/SSE/etc reg access)
        let em @ 2; // Emulation (trap on FPU/MMX/SSE/etc instructions)
        let mp @ 1; // Monitor Coprocessor
        let pe @ 0; // Protection Enable (enable protected mode)
    }
});

// There is no CR1.

/// [intel/vol3]: 2.5 Control Registers: CR2
pub const CR2: Cr<2, ControlRegister2> = Cr::new();

layout!({
    /// The layout of [`CR2`].
    pub struct ControlRegister2(u64);
    {
        let address @ 63..0;
    }
});

/// [intel/vol3]: 2.5 Control Registers: CR3 without PCID feature
pub const CR3: Cr<3, ControlRegister3> = Cr::new();

/// [intel/vol3]: 2.5 Control Registers: CR3 with CR4.PCIDE enabled
pub const CR3_PCID: Cr<3, ControlRegister3Pcid> = Cr::new();

layout!({
    /// The layout of [`CR3`].
    pub struct ControlRegister3(u64);
    {
        let __ @ 63..52;
        #[unshifted]
        let base @ 51..12; // 4k-aligned physical byte address.

        // Bits [12:5] and [2:0] are reserved and ignored, but "assumed to be zero".
        // In case of future additions it's probably best to write them back as
        // written rather than RSVDZ them.
        let __ @ 11..5;
        let pcd @ 4; // Page-level Cache Disable
        let pwt @ 3; // Page-level Write-Through
        let __ @ 2..0;
    }
});

layout!({
    /// The layout of [`CR3_PCID`].
    pub struct ControlRegister3Pcid(u64);
    {
        let noflush @ 63; // When this bit is set upon load, do not flush the PCID's TLB.
        let __ @ 62..52;
        #[unshifted]
        let base @ 51..12; // 4k-aligned physical byte address.
        let pcid @ 11..0; // 12 bit PCID associated with this mmu context
    }
});

/// [intel/vol3]: 2.5 Control Registers: CR4
pub const CR4: Cr<4, ControlRegister4> = Cr::new();

layout!({
    /// The layout of [`CR4`].
    pub struct ControlRegister4(u64);
    {
        let __ @ 63..32 = 0;
        let __ @ 31..25;
        let pks @ 24; // Enable protection keys for supervisor-mode pages
        let cet @ 23; // Control-flow Enforcement Technology
        let pke @ 22; // Enable protection keys for user-mode pages
        let smap @ 21; // SMAP-Enable Bit
        let smep @ 20; // SMEP-Enable Bit
        let __ @ 19;
        let osxsave @ 18; // XSAVE and Processor Extended States-Enable Bit
        let pcide @ 17; // PCID-Enable Bit
        let fsgsbase @ 16; // FSGSBASE-Enable Bit
        let __ @ 15;
        let smxe @ 14; // SMX-Enable Bit
        let vmxe @ 13; // VMX-Enable Bit
        let la57 @ 12; // 57-bit linear addresses
        let umip @ 11; // User-Mode Instruction Prevention
        let osmmexcpt @ 10; // OS supports unmasked SIMD FP Exceptions
        let osfxsr @ 9; // OS supports FXSAVE and FXRSTOR
        let pce @ 8; // Performance-Monitoring Counter Enable
        let pge @ 7; // Page Global Enable
        let mce @ 6; // Machine-Check Enable
        let pae @ 5; // Physical Address Extension
        let pse @ 4; // Page Size Extensions
        let de @ 3; // Debugging Extensions
        let tsd @ 2; // Time Stamp Disable
        let pvi @ 1; // Protected-Mode Virtual Interrupts
        let vme @ 0; // Virtual-8086 Mode Extensions
    }
});

// There is no CR5, CR6, or CR7.

/// [intel/vol3]: 2.5 Control Registers: CR8
pub const CR8: Cr<8, ControlRegister8> = Cr::new();

layout!({
    /// The layout of [`CR8`].
    pub struct ControlRegister8(u64);
    {
        let __ @ 63..4;
        let tpl @ 3..0; // Task Priority Level
    }
});

layout!({
    /// [intel/vol3]: 2.6 Extended Control Registers
    ///
    /// The layout of extended control register 0 (XCR0).
    pub struct ExtendedControlRegister0(u64);
    {
        // Bit 63 of XCR0 is reserved for future expansion and will not represent a
        // processor state component.
        let __ @ 63 = 0;
        let __ @ 62..10 = 0; // Reserved for future expansion.
        let pkru @ 9;
        let __ @ 8 = 0;
        let hi16_zmm @ 7;
        let zmm_hi256 @ 6;
        let opmask @ 5;
        let bndcsr @ 4;
        let bndreg @ 3;
        let avx @ 2;
        let sse @ 1;
        let x87 @ 0;
    }
});
