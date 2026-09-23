// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::profile_dispatcher::{ProfileDispatcher, ProfileDispatcherState};
use core::mem::MaybeUninit;
use zx_types::{zx_profile_info_t, zx_status_t};

unsafe extern "C" {
    pub(crate) fn cpp_profile_dispatcher_create(
        info: *const zx_profile_info_t,
        handle_out: *mut MaybeUninit<KernelHandle<ProfileDispatcher>>,
    ) -> zx_status_t;

    pub(crate) fn cpp_profile_dispatcher_validate_and_create_profile(
        info: *const zx_profile_info_t,
        profile_out: *mut MaybeUninit<crate::kernel::scheduler_state::SchedulerStateBaseProfile>,
    ) -> zx_status_t;
}

super::dispatcher::impl_dispatcher_state_init!(
    ProfileDispatcher,
    ProfileDispatcherState,
    info: &zx_profile_info_t,
);
