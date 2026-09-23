// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::kernel::deadline::Deadline;
use crate::kernel::thread::{Interruptible, sleep_etc};
use crate::object::{AutoBlocked, Blocked, ProcessDispatcher};
use crate::platform_rs::timer::{InstantUnknown, current_mono_time};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::zx_instant_mono_t;

const LOCAL_TRACE: u32 = 0;

define_kcounter!(SYSCALLS_ZX_NANOSLEEP, "syscalls.zx_nanosleep", Sum);
define_kcounter!(SYSCALLS_ZX_NANOSLEEP_ZERO_DURATION, "syscalls.zx_nanosleep_zero_duration", Sum);

#[syscall]
pub fn sys_nanosleep(deadline: zx_instant_mono_t) -> Result<(), Status> {
    ltracef!("nseconds {}\n", deadline);
    SYSCALLS_ZX_NANOSLEEP.add(1);

    if deadline <= 0 {
        SYSCALLS_ZX_NANOSLEEP_ZERO_DURATION.add(1);
        return Ok(());
    }

    let now = current_mono_time();
    let slack = ProcessDispatcher::with_current(|up| up.get_timer_slack_policy());
    let slack_deadline = Deadline::new(InstantUnknown(deadline), slack);

    let _by = AutoBlocked::new(Blocked::SLEEPING);

    // This syscall is declared as "blocking", so a higher layer will automatically
    // retry if we return ZX_ERR_INTERNAL_INTR_RETRY.
    sleep_etc(&slack_deadline, Interruptible::YES, now.0)
}
