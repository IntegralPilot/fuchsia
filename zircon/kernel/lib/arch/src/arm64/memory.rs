// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitrs::layout;
use regio::arm64::{SysReg, spec};

/// [arm/v8]: D13.2.95  MAIR_EL1, Memory Attribute Indirection Register, EL1
pub const MAIR_EL1: SysReg<spec::MAIR_EL1, MemoryAttrIndirectionRegister> = SysReg::new();

/// [arm/v8]: D13.2.96  MAIR_EL2, Memory Attribute Indirection Register, EL2
pub const MAIR_EL2: SysReg<spec::MAIR_EL2, MemoryAttrIndirectionRegister> = SysReg::new();

layout!({
    /// The layout of [`MAIR_EL1`] and [`MAIR_EL2`].
    pub struct MemoryAttrIndirectionRegister(u64);
    {
        let attr7 @ 63..56;
        let attr6 @ 55..48;
        let attr5 @ 47..40;
        let attr4 @ 39..32;
        let attr3 @ 31..24;
        let attr2 @ 23..16;
        let attr1 @ 15..8;
        let attr0 @ 7..0;
    }
});
