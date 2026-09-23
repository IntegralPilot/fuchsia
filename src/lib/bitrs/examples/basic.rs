// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::layout;

// Models the x86 EFLAGS register.
layout!({
    pub struct Eflags(u32);
    {
        let __ @ 31..22 = 0;
        let id @ 21; // ID flag
        let vip @ 20; // Virtual interrupt pending
        let vif @ 19; // Virtual interrupt
        let ac @ 18; // Alignment check / access control
        let vm @ 17; // Virtual-8086 mode
        let rf @ 16; // Resume flag
        let __ @ 15 = 0;
        let nt @ 14; // Nested task
        let iopl @ 13..12; // I/O privilege level
        let of @ 11; // Overflow flag
        let df @ 10; // Direction flag
        let if_ @ 9 = 1; // Interrupt enable flag
        let tf @ 8; // Trap flag
        let sf @ 7; // Sign flag
        let zf @ 6; // Zero flag
        let __ @ 5 = 0;
        let af @ 4; // Auxiliary carry flag
        let __ @ 3 = 0;
        let pf @ 2; // Parity flag
        let __ @ 1 = 1;
        let cf @ 0; // Carry flag
    }
});

fn main() {
    macro_rules! print_formatted {
        ($obj:ident) => {
            let obj_name = stringify!($obj);
            println!("{obj_name}: debug: {:?}", $obj);
            println!("{obj_name}: lower hex: {:x}", $obj);
            println!("{obj_name}: upper hex: {:X}", $obj);
            println!("{obj_name}: octal: {:o}", $obj);
        };
    }

    let new = Eflags::new();
    let default = Eflags::default();
    let custom = *Eflags::new().set_iopl(0b11).set_if_(true).set_zf(true).set_cf(true);

    print_formatted!(new);
    print_formatted!(default);
    print_formatted!(custom);
}
