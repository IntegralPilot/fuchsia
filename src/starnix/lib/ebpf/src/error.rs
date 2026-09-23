// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum EbpfError {
    #[error("Unable to create VM")]
    VmInitialization,

    #[error("VM error registering callback: {0}")]
    VmRegisterError(String),

    #[error("Verification error loading program: {0}")]
    ProgramVerifyError(String),

    #[error("Failed to link program: {0}")]
    ProgramLinkError(String),

    #[error("VM error loading program: {0}")]
    VmLoadError(String),

    #[error("Invalid cBPF instruction code: 0x{0:x}")]
    InvalidCbpfInstruction(u16),

    #[error("Invalid cBPF scratch memory offset: 0x{0:x}")]
    InvalidCbpfScratchOffset(u32),

    #[error("Invalid cBPF jump offset: 0x{0:x}")]
    InvalidCbpfJumpOffset(u32),
}
