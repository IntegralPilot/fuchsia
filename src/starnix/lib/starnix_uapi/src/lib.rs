// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod arc_key;
pub mod as_any;
pub mod auth;
pub mod device_id;
pub mod elf;
pub mod errors;
pub mod file_lease;
pub mod file_mode;
pub mod fs_type;
pub mod inotify_mask;
pub mod iptables_flags;
pub mod kcmp;
pub mod math;
pub mod mount_flags;
pub mod open_flags;
pub mod personality;
pub mod range_ext;
pub mod resource_limits;
pub mod restricted_aspace;
pub mod seal_flags;
pub mod selinux;
pub mod signals;
pub mod syslog;
pub mod termios;
pub mod uapi;
pub mod union;
pub mod unmount_flags;
pub mod user_address;
pub mod user_value;
pub mod version;
pub mod vfs;

#[cfg(target_arch = "aarch64")]
pub mod arm64;

#[cfg(target_arch = "aarch64")]
pub mod arm;

#[cfg(target_arch = "aarch64")]
pub use arm64::*;

#[cfg(target_arch = "x86_64")]
pub mod x64;

#[cfg(target_arch = "x86_64")]
pub use x64::*;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;

#[cfg(target_arch = "riscv64")]
#[allow(unused_imports)]
pub use riscv64::*;

pub mod arch32 {

    #[cfg(target_arch = "aarch64")]
    pub use super::arm::*;

    #[cfg(target_arch = "x86_64")]
    pub use super::x64::*;

    #[cfg(target_arch = "riscv64")]
    #[allow(unused_imports)]
    pub use super::riscv64::*;

    pub use super::uapi::arch32::*;
}

pub use uapi::*;

#[doc(hidden)]
pub use zx_status as __zx_status;
