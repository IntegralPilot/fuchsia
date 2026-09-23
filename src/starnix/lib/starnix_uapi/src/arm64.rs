// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_camel_case_types)]

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
pub struct user_regs_struct {
    pub regs: [u64; 31usize],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
}

#[repr(C)]
#[repr(align(16))]
#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
pub struct user_fpsimd_struct {
    pub vregs: [crate::__uint128_t; 32usize],
    pub fpsr: u32,
    pub fpcr: u32,
    pub _padding_0: [u8; 8usize],
}
