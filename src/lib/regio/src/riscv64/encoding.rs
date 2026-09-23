// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_camel_case_types)]

//! This module stamps out [`CsrEncoding`] instances corresponding to known
//! riscv64 CSRs, named by their official mnemonic (e.g., `sstatus`).

use super::CsrEncoding;
use crate::{Ro, RwSafe, RwUnsafe};

// This information is primarily found in the "Control and Status Registers (CSRs)"
// chapter of Volume II: RISC-V Privileged Architectures and Volume I: Unprivileged
// Architecture in the RISC-V Instruction Set Manual.
//
// The usage of RwSafe vs. RwUnsafe depends on the nature of the particular
// CSR.
macro_rules! for_each_encoding {
    ($macro:path) => {
        //
        // Unprivileged Floating-Point CSRs
        //

        $macro!(fflags, 0x001, RwSafe);
        $macro!(frm, 0x002, RwSafe);
        $macro!(fcsr, 0x003, RwSafe);

        //
        // Unprivileged Vector CSRs
        //

        $macro!(vstart, 0x008, RwSafe);
        $macro!(vxsat, 0x009, RwSafe);
        $macro!(vxrm, 0x00a, RwSafe);
        $macro!(vcsr, 0x00f, RwSafe);
        $macro!(vl, 0xc20, Ro);
        $macro!(vtype, 0xc21, Ro);
        $macro!(vlenb, 0xc22, Ro);

        //
        // Unprivileged Zicfiss extension CSR
        //

        // Can change active shadow stack pointer.
        $macro!(ssp, 0x011, RwUnsafe);

        //
        // Unprivileged Entropy Source Extension CSR
        //

        $macro!(seed, 0x015, RwSafe);

        //
        // Unprivileged Zcmt Extension CSR
        //

        // Can change jump table base address.
        $macro!(jvt, 0x017, RwUnsafe);

        //
        // Unprivileged Counter/Timers
        //

        $macro!(cycle, 0xc00, Ro);
        $macro!(time, 0xc01, Ro);
        $macro!(instret, 0xc02, Ro);
        $macro!(hpmcounter3, 0xc03, Ro);
        $macro!(hpmcounter4, 0xc04, Ro);
        $macro!(hpmcounter31, 0xc1f, Ro);
        $macro!(cycleh, 0xc80, Ro);
        $macro!(timeh, 0xc81, Ro);
        $macro!(instreth, 0xc82, Ro);
        $macro!(hpmcounter3h, 0xc83, Ro);
        $macro!(hpmcounter4h, 0xc84, Ro);
        $macro!(hpmcounter31h, 0xc9f, Ro);

        //
        // Supervisor Trap Setup
        //

        $macro!(sstatus, 0x100, RwSafe);
        $macro!(sie, 0x104, RwSafe);
        $macro!(stvec, 0x105, RwSafe);
        $macro!(scounteren, 0x106, RwSafe);

        //
        // Supervisor Configuration
        //

        $macro!(senvcfg, 0x10a, RwSafe);

        //
        // Supervisor Counter Setup
        //

        $macro!(scountinhibit, 0x120, RwSafe);

        //
        // Supervisor Trap Handling
        //

        $macro!(sscratch, 0x140, RwSafe);
        // Can change the return address.
        $macro!(sepc, 0x141, RwUnsafe);

        $macro!(scause, 0x142, RwSafe);
        $macro!(stval, 0x143, RwSafe);
        $macro!(sip, 0x144, RwSafe);
        $macro!(scountovf, 0xda0, Ro);

        //
        // Supervisor Indirect
        //

        $macro!(siselect, 0x150, RwSafe);
        $macro!(sireg, 0x151, RwSafe);
        $macro!(sireg2, 0x152, RwSafe);
        $macro!(sireg3, 0x153, RwSafe);
        $macro!(sireg4, 0x155, RwSafe);
        $macro!(sireg5, 0x156, RwSafe);
        $macro!(sireg6, 0x157, RwSafe);

        //
        // Supervisor Protection and Translation
        //

        // Can change memory mappings.
        $macro!(satp, 0x180, RwUnsafe);

        //
        // Supervisor Timer Compare
        //

        $macro!(stimecmp, 0x14d, RwSafe);
        $macro!(stimecmph, 0x15d, RwSafe);

        //
        // Debug/Trace Registers
        //

        $macro!(scontext, 0x5a8, RwSafe);

        //
        // Supervisor Resource Management Configuration
        //

        $macro!(srmcfg, 0x181, RwSafe);

        //
        // Supervisor State Enable Registers
        //

        $macro!(sstateen0, 0x10c, RwSafe);
        $macro!(sstateen1, 0x10d, RwSafe);
        $macro!(sstateen2, 0x10e, RwSafe);
        $macro!(sstateen3, 0x10f, RwSafe);

        //
        // Supervisor Control Transfer Records Configuration
        //

        $macro!(sctrctl, 0x14e, RwSafe);
        $macro!(sctrstatus, 0x14f, RwSafe);
        $macro!(sctrdepth, 0x15f, RwSafe);

        //
        // Hypervisor Trap Setup
        //

        $macro!(hstatus, 0x600, RwSafe);
        $macro!(hedeleg, 0x602, RwSafe);
        $macro!(hideleg, 0x603, RwSafe);
        $macro!(hie, 0x604, RwSafe);
        $macro!(hcounteren, 0x606, RwSafe);
        $macro!(hgeie, 0x607, RwSafe);
        $macro!(hedelegh, 0x612, RwSafe);

        //
        // Hypervisor Trap Handling
        //

        $macro!(htval, 0x643, RwSafe);
        $macro!(hip, 0x644, RwSafe);
        $macro!(hvip, 0x645, RwSafe);
        $macro!(htinst, 0x64a, RwSafe);
        $macro!(hgeip, 0xe12, Ro);

        //
        // Hypervisor Configuration
        //

        $macro!(henvcfg, 0x60a, RwSafe);
        $macro!(henvcfgh, 0x61a, RwSafe);

        //
        // Hypervisor Protection and Translation
        //

        // Safe-writable, since the writer's execution context is not subject to
        // the related guest memory mappings.
        $macro!(hgatp, 0x680, RwSafe);

        //
        // Debug/Trace Registers
        //

        $macro!(hcontext, 0x6a8, RwSafe);

        //
        // Hypervisor Counter/Timer Virtualization Registers
        //

        $macro!(htimedelta, 0x605, RwSafe);
        $macro!(htimedeltah, 0x615, RwSafe);

        //
        // Hypervisor State Enable Registers
        //

        $macro!(hstateen0, 0x60c, RwSafe);
        $macro!(hstateen1, 0x60d, RwSafe);
        $macro!(hstateen2, 0x60e, RwSafe);
        $macro!(hstateen3, 0x60f, RwSafe);
        $macro!(hstateen0h, 0x61c, RwSafe);
        $macro!(hstateen1h, 0x61d, RwSafe);
        $macro!(hstateen2h, 0x61e, RwSafe);
        $macro!(hstateen3h, 0x61f, RwSafe);

        //
        // Virtual Supervisor Registers
        //

        $macro!(vsstatus, 0x200, RwSafe);
        $macro!(vsie, 0x204, RwSafe);
        $macro!(vstvec, 0x205, RwSafe);
        $macro!(vsscratch, 0x240, RwSafe);

        // Can change the return address.
        $macro!(vsepc, 0x241, RwUnsafe);
        $macro!(vscause, 0x242, RwSafe);
        $macro!(vstval, 0x243, RwSafe);
        $macro!(vsip, 0x244, RwSafe);

        // Can change memory mappings.
        $macro!(vsatp, 0x280, RwUnsafe);

        //
        // Virtual Supervisor Indirect
        //

        $macro!(vsiselect, 0x250, RwSafe);
        $macro!(vsireg, 0x251, RwSafe);
        $macro!(vsireg2, 0x252, RwSafe);
        $macro!(vsireg3, 0x253, RwSafe);
        $macro!(vsireg4, 0x255, RwSafe);
        $macro!(vsireg5, 0x256, RwSafe);
        $macro!(vsireg6, 0x257, RwSafe);

        //
        // Virtual Supervisor Timer Compare
        //

        $macro!(vstimecmp, 0x24d, RwSafe);
        $macro!(vstimecmph, 0x25d, RwSafe);

        //
        // Virtual Supervisor Control Transfer Records Configuration
        //

        $macro!(vsctrctl, 0x24e, RwSafe);

        //
        // Machine Information Registers
        //

        $macro!(mvendorid, 0xf11, Ro);
        $macro!(marchid, 0xf12, Ro);
        $macro!(mimpid, 0xf13, Ro);
        $macro!(mhartid, 0xf14, Ro);
        $macro!(mconfigptr, 0xf15, Ro);

        //
        // Machine Trap Setup
        //

        $macro!(mstatus, 0x300, RwSafe);
        $macro!(misa, 0x301, RwSafe);
        $macro!(medeleg, 0x302, RwSafe);
        $macro!(mideleg, 0x303, RwSafe);
        $macro!(mie, 0x304, RwSafe);
        $macro!(mtvec, 0x305, RwSafe);
        $macro!(mcounteren, 0x306, RwSafe);
        $macro!(mstatush, 0x310, RwSafe);
        $macro!(medelegh, 0x312, RwSafe);

        //
        // Machine Trap Handling
        //

        $macro!(mscratch, 0x340, RwSafe);

        // Can change the return address.
        $macro!(mepc, 0x341, RwUnsafe);
        $macro!(mcause, 0x342, RwSafe);
        $macro!(mtval, 0x343, RwSafe);
        $macro!(mip, 0x344, RwSafe);
        $macro!(mtinst, 0x34a, RwSafe);
        $macro!(mtval2, 0x34b, RwSafe);

        //
        // Machine Indirect
        //

        $macro!(miselect, 0x350, RwSafe);
        $macro!(mireg, 0x351, RwSafe);
        $macro!(mireg2, 0x352, RwSafe);
        $macro!(mireg3, 0x353, RwSafe);
        $macro!(mireg4, 0x355, RwSafe);
        $macro!(mireg5, 0x356, RwSafe);
        $macro!(mireg6, 0x357, RwSafe);

        //
        // Machine Configuration
        //

        $macro!(menvcfg, 0x30a, RwSafe);
        $macro!(menvcfgh, 0x31a, RwSafe);
        $macro!(mseccfg, 0x747, RwSafe);
        $macro!(mseccfgh, 0x757, RwSafe);

        //
        // Machine Memory Protection
        //

        $macro!(pmpcfg0, 0x3a0, RwSafe);
        $macro!(pmpcfg1, 0x3a1, RwSafe);
        $macro!(pmpcfg2, 0x3a2, RwSafe);
        $macro!(pmpcfg3, 0x3a3, RwSafe);
        $macro!(pmpcfg14, 0x3ae, RwSafe);
        $macro!(pmpcfg15, 0x3af, RwSafe);
        $macro!(pmpaddr0, 0x3b0, RwSafe);
        $macro!(pmpaddr1, 0x3b1, RwSafe);
        $macro!(pmpaddr63, 0x3ef, RwSafe);

        //
        // Machine State Enable Registers
        //

        $macro!(mstateen0, 0x30c, RwSafe);
        $macro!(mstateen1, 0x30d, RwSafe);
        $macro!(mstateen2, 0x30e, RwSafe);
        $macro!(mstateen3, 0x30f, RwSafe);
        $macro!(mstateen0h, 0x31c, RwSafe);
        $macro!(mstateen1h, 0x31d, RwSafe);
        $macro!(mstateen2h, 0x31e, RwSafe);
        $macro!(mstateen3h, 0x31f, RwSafe);

        //
        // Machine Non-Maskable Interrupt Handling
        //

        $macro!(mnscratch, 0x740, RwSafe);

        // Can change the return address.
        $macro!(mnepc, 0x741, RwUnsafe);

        $macro!(mncause, 0x742, RwSafe);
        $macro!(mnstatus, 0x744, RwSafe);

        //
        // Machine Counter/Timers
        //

        $macro!(mcycle, 0xb00, RwSafe);
        $macro!(minstret, 0xb02, RwSafe);
        $macro!(mhpmcounter3, 0xb03, RwSafe);
        $macro!(mhpmcounter4, 0xb04, RwSafe);
        $macro!(mhpmcounter31, 0xb1f, RwSafe);
        $macro!(mcycleh, 0xb80, RwSafe);
        $macro!(minstreth, 0xb82, RwSafe);
        $macro!(mhpmcounter3h, 0xb83, RwSafe);
        $macro!(mhpmcounter4h, 0xb84, RwSafe);
        $macro!(mhpmcounter31h, 0xb9f, RwSafe);

        //
        // Machine Counter Setup
        //

        $macro!(mcountinhibit, 0x320, RwSafe);
        $macro!(mcyclecfg, 0x321, RwSafe);
        $macro!(minstretcfg, 0x322, RwSafe);
        $macro!(mhpmevent3, 0x323, RwSafe);
        $macro!(mhpmevent4, 0x324, RwSafe);
        $macro!(mhpmevent31, 0x33f, RwSafe);
        $macro!(mcyclecfgh, 0x721, RwSafe);
        $macro!(minstretcfgh, 0x722, RwSafe);
        $macro!(mhpmevent3h, 0x723, RwSafe);
        $macro!(mhpmevent4h, 0x724, RwSafe);
        $macro!(mhpmevent31h, 0x73f, RwSafe);

        //
        // Machine Control Transfer Records Configuration
        //

        $macro!(mctrctl, 0x34e, RwSafe);

        //
        // Debug/Trace Registers (shared with Debug Mode)
        //

        $macro!(tselect, 0x7a0, RwSafe);
        $macro!(tdata1, 0x7a1, RwSafe);
        $macro!(tdata2, 0x7a2, RwSafe);
        $macro!(tdata3, 0x7a3, RwSafe);
        $macro!(mcontext, 0x7a8, RwSafe);

        //
        // Debug Mode Registers
        //

        $macro!(dcsr, 0x7b0, RwSafe);

        // Can change the return address.
        $macro!(dpc, 0x7b1, RwUnsafe);

        $macro!(dscratch0, 0x7b2, RwSafe);
        $macro!(dscratch1, 0x7b3, RwSafe);
    };
}

macro_rules! define_encoding {
    ($name:ident, $encoding:literal, $access:ty) => {
        pub type $name = CsrEncoding<$encoding, $access>;
    };
}

for_each_encoding!(define_encoding);
