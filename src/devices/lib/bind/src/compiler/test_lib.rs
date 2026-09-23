// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::compiler::{Symbol, SymbolicInstruction, SymbolicInstructionInfo};

pub fn make_abort_ne_symbinst<'a>(lhs: Symbol, rhs: Symbol) -> SymbolicInstructionInfo<'a> {
    SymbolicInstructionInfo {
        location: None,
        instruction: SymbolicInstruction::AbortIfNotEqual { lhs: lhs, rhs: rhs },
    }
}

pub fn make_abort_eq_symbinst<'a>(lhs: Symbol, rhs: Symbol) -> SymbolicInstructionInfo<'a> {
    SymbolicInstructionInfo {
        location: None,
        instruction: SymbolicInstruction::AbortIfEqual { lhs: lhs, rhs: rhs },
    }
}
