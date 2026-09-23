// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub type c_void = ::std::ffi::c_void;

// `char` is signed on x86_64, but unsigned on arm64 and riscv.
#[cfg(target_arch = "x86_64")]
pub type c_char = i8;
#[cfg(any(target_arch = "arm", target_arch = "aarch64", target_arch = "riscv64"))]
pub type c_char = u8;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64"))]
mod common {
    pub type c_schar = i8;
    pub type c_uchar = u8;
    pub type c_short = i16;
    pub type c_ushort = u16;
    pub type c_int = i32;
    pub type c_uint = u32;
    pub type c_long = i64;
    pub type c_ulong = u64;
    pub type c_longlong = i64;
    pub type c_ulonglong = u64;
}

// Export for 'arm' which is only used via arch32
#[allow(dead_code)]
pub mod arch32 {
    pub type c_char = u8;

    pub type c_schar = i8;
    pub type c_uchar = u8;
    pub type c_short = i16;
    pub type c_ushort = u16;
    pub type c_int = i32;
    pub type c_uint = u32;
    pub type c_long = i32;
    pub type c_ulong = u32;
    pub type c_longlong = i64;
    pub type c_ulonglong = u64;
}

pub use common::*;
