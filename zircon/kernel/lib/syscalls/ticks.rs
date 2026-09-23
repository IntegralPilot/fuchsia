// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use syscalls_macro::syscall;
use zx_types::{zx_instant_boot_ticks_t, zx_instant_mono_ticks_t};

define_kcounter!(SYSCALLS_ZX_TICKS_GET, "syscalls.zx_ticks_get", Sum);
define_kcounter!(SYSCALLS_ZX_TICKS_GET_BOOT, "syscalls.zx_ticks_get_boot", Sum);

#[syscall]
pub fn sys_ticks_get_via_kernel() -> zx_instant_mono_ticks_t {
    SYSCALLS_ZX_TICKS_GET.add(1);
    crate::platform_rs::timer::timer_current_mono_ticks().0
}

#[syscall]
pub fn sys_ticks_get_boot_via_kernel() -> zx_instant_boot_ticks_t {
    SYSCALLS_ZX_TICKS_GET_BOOT.add(1);
    crate::platform_rs::timer::timer_current_boot_ticks().0
}
