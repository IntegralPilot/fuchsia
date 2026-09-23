// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use syscalls_macro::syscall;
use zx_types::{zx_instant_boot_t, zx_instant_mono_t};

define_kcounter!(SYSCALLS_ZX_CLOCK_GET_MONOTONIC, "syscalls.zx_clock_get_monotonic", Sum);
define_kcounter!(SYSCALLS_ZX_CLOCK_GET_BOOT, "syscalls.zx_clock_get_boot", Sum);

#[syscall]
pub fn sys_clock_get_monotonic_via_kernel() -> zx_instant_mono_t {
    SYSCALLS_ZX_CLOCK_GET_MONOTONIC.add(1);
    crate::platform_rs::timer::current_mono_time().0
}

#[syscall]
pub fn sys_clock_get_boot_via_kernel() -> zx_instant_boot_t {
    SYSCALLS_ZX_CLOCK_GET_BOOT.add(1);
    crate::platform_rs::timer::current_boot_time().0
}
