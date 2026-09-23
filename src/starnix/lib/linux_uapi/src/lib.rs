// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

mod types;
pub use types::*;

mod manual;
pub use manual::*;

pub mod macros;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(target_arch = "aarch64")]
pub mod arm64;

#[cfg(target_arch = "aarch64")]
pub use arm64::*;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;

#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

// Bring in arm under 'arch32' for aarch64.
#[cfg(all(target_arch = "aarch64"))]
mod arm;
#[cfg(all(target_arch = "aarch64"))]
mod arm_manual;
#[cfg(all(target_arch = "aarch64"))]
mod arm_manual_kgsl;

#[cfg(all(target_arch = "aarch64"))]
pub mod arch32 {
    pub use crate::arm::*;
}

#[cfg(not(target_arch = "aarch64"))]
mod arch32_stub;
#[cfg(not(target_arch = "aarch64"))]
pub mod arch32 {
    pub use crate::arch32_stub::*;
}

#[doc(hidden)]
pub use static_assertions as __static_assertions;
