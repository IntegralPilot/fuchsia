// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// From //zircon/kernel/arch/x86/include/arch/kernel_aspace.h
#[cfg(target_arch = "x86_64")]
const USER_ASPACE_BASE: usize = 0x0000000000200000;
#[cfg(target_arch = "x86_64")]
const USER_RESTRICTED_ASPACE_SIZE: usize = (1 << 46) - USER_ASPACE_BASE;

// From //zircon/kernel/arch/arm64/include/arch/kernel_aspace.h
#[cfg(target_arch = "aarch64")]
const USER_ASPACE_BASE: usize = 0x0000000000200000;
#[cfg(target_arch = "aarch64")]
const USER_RESTRICTED_ASPACE_SIZE: usize = (1 << 47) - USER_ASPACE_BASE;

// From //zircon/kernel/arch/riscv64/include/arch/kernel_aspace.h
#[cfg(target_arch = "riscv64")]
const USER_ASPACE_BASE: usize = 0x0000000000200000;
#[cfg(target_arch = "riscv64")]
const USER_RESTRICTED_ASPACE_SIZE: usize = (1 << 37) - USER_ASPACE_BASE;

// From //zircon/kernel/object/process_dispatcher.cc
pub const RESTRICTED_ASPACE_BASE: usize = USER_ASPACE_BASE;
pub const RESTRICTED_ASPACE_SIZE: usize = USER_RESTRICTED_ASPACE_SIZE;
pub const RESTRICTED_ASPACE_HIGHEST_ADDRESS: usize =
    RESTRICTED_ASPACE_BASE + RESTRICTED_ASPACE_SIZE;

pub const RESTRICTED_ASPACE_RANGE: std::ops::Range<usize> =
    RESTRICTED_ASPACE_BASE..RESTRICTED_ASPACE_HIGHEST_ADDRESS;
