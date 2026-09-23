// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2009 Corey Tabaka
// Copyright (c) 2015 Intel Corporation
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::arch_rs::x86::x86::outp;

/// Master PIC (PIC1) command and status I/O port.
pub const PIC1_COMMAND: u16 = 0x20;
/// Master PIC (PIC1) data and interrupt mask register I/O port.
pub const PIC1_DATA: u16 = 0x21;

/// Slave PIC (PIC2) command and status I/O port.
pub const PIC2_COMMAND: u16 = 0xA0;
/// Slave PIC (PIC2) data and interrupt mask register I/O port.
pub const PIC2_DATA: u16 = 0xA1;

/// Initialization Command Word 1 (ICW1): Initialization bit (0x10) + ICW4 needed (0x01).
pub const ICW1: u8 = 0x11;

/// Initialization Command Word 3 (ICW3) for Master: Slave controller connected to IRQ2 (bit 2 = 0x04).
pub const ICW3_MASTER: u8 = 0x04;
/// Initialization Command Word 3 (ICW3) for Slave: Slave identification code (IRQ2 = 0x02).
pub const ICW3_SLAVE: u8 = 0x02;

/// Initialization Command Word 4 (ICW4): 8086/88 mode (0x01).
pub const ICW4_8086: u8 = 0x01;
/// Initialization Command Word 4 (ICW4) for Master: 8086/88 mode + special fully nested / buffered mode (0x05).
pub const ICW4_MASTER: u8 = 0x05;

/// Operation Control Word 1 (OCW1): Mask all 8 IRQs on the PIC.
pub const PIC_DISABLE_ALL_IRQS: u8 = 0xFF;

/// Initializes the 8259A PICs and remaps their base interrupt vectors.
///
/// Sends the 4-step Initialization Command Word (ICW) sequence to both the master
/// and slave PICs to remap their interrupt vectors to `pic1` and `pic2` respectively,
/// configure master/slave cascading on IRQ2, set 8086 mode, and mask all IRQs.
pub fn pic_map(pic1: u8, pic2: u8) {
    /* send ICW1 */
    // SAFETY: Initializing master and slave PICs by writing ICW1 to their command ports.
    unsafe {
        outp(PIC1_COMMAND, ICW1);
        outp(PIC2_COMMAND, ICW1);
    }

    /* send ICW2 */
    // SAFETY: Writing base interrupt vector offsets (ICW2) to master and slave PIC data ports.
    unsafe {
        outp(PIC1_DATA, pic1); /* remap */
        outp(PIC2_DATA, pic2); /*  pics */
    }

    /* send ICW3 */
    // SAFETY: Configuring cascade topology (ICW3): master has slave on IRQ2; slave is cascaded to IRQ2.
    unsafe {
        outp(PIC1_DATA, ICW3_MASTER); /* IRQ2 -> connection to slave */
        outp(PIC2_DATA, ICW3_SLAVE);
    }

    /* send ICW4 */
    // SAFETY: Configuring operation modes (ICW4) on master and slave PICs.
    unsafe {
        outp(PIC1_DATA, ICW4_MASTER);
        outp(PIC2_DATA, ICW4_8086);
    }

    /* disable all IRQs */
    // SAFETY: Masking all interrupt lines on both PICs by writing 0xFF to their data registers (OCW1).
    unsafe {
        outp(PIC1_DATA, PIC_DISABLE_ALL_IRQS);
        outp(PIC2_DATA, PIC_DISABLE_ALL_IRQS);
    }
}

/// Disables all interrupt lines on both PICs by setting their interrupt masks to 0xFF.
pub fn pic_disable() {
    // SAFETY: Writing 0xFF to slave and master PIC data ports to mask all IRQs.
    unsafe {
        outp(PIC2_DATA, PIC_DISABLE_ALL_IRQS);
        outp(PIC1_DATA, PIC_DISABLE_ALL_IRQS);
    }
}
