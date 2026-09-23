// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::Errno;
use crate::{error, uapi};
use inspect_stubs::track_stub;

#[derive(Clone, Copy, PartialEq)]
#[repr(usize)]
pub enum ElfNoteType {
    PrStatus = uapi::NT_PRSTATUS as usize,
    FpRegSet = uapi::NT_PRFPREG as usize,
    X86XState = uapi::NT_X86_XSTATE as usize,
    ArmTaggedAddrCtrl = uapi::NT_ARM_TAGGED_ADDR_CTRL as usize,
    ArmPacEnabledKeys = uapi::NT_ARM_PAC_ENABLED_KEYS as usize,
}

impl TryFrom<usize> for ElfNoteType {
    type Error = Errno;

    fn try_from(v: usize) -> Result<Self, Errno> {
        match v {
            x if x == ElfNoteType::PrStatus as usize => Ok(ElfNoteType::PrStatus),
            x if x == ElfNoteType::FpRegSet as usize => Ok(ElfNoteType::FpRegSet),
            x if x == ElfNoteType::X86XState as usize => Ok(ElfNoteType::X86XState),
            x if x == ElfNoteType::ArmTaggedAddrCtrl as usize => {
                track_stub!(TODO("https://fxbug.dev/441149562"), "NT_ARM_TAGGED_ADDR_CTRL");
                error!(ENOTSUP)
            }
            x if x == ElfNoteType::ArmPacEnabledKeys as usize => {
                track_stub!(TODO("https://fxbug.dev/441149562"), "NT_ARM_PAC_ENABLED_KEYS");
                error!(ENOTSUP)
            }
            _ => error!(EINVAL),
        }
    }
}
