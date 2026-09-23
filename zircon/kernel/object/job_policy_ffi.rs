// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::job_policy::JobPolicy;
use crate::kernel::deadline::TimerSlack;
use zx_types::{ZX_OK, zx_policy_basic_v2_t, zx_status_t};

/// # Safety
///
/// `out` must point to valid storage for a [`JobPolicy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_job_policy_create_root_policy(out: *mut JobPolicy) {
    // SAFETY: The caller guarantees `out` points to valid storage for `JobPolicy`.
    unsafe {
        core::ptr::write(out, JobPolicy::create_root_policy());
    }
}

/// # Safety
///
/// If `policy_count > 0`, `policy_input` must point to an array of `policy_count` initialized
/// `zx_policy_basic_v2_t` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_job_policy_add_basic_policy(
    policy: &mut JobPolicy,
    mode: u32,
    policy_input: *const zx_policy_basic_v2_t,
    policy_count: usize,
) -> zx_status_t {
    // SAFETY: The caller guarantees `policy_input` points to `policy_count` initialized elements
    // whenever `policy_count > 0`.
    let input = unsafe { zr::slice_from_raw_parts(policy_input, policy_count) };
    match policy.add_basic_policy(mode, input) {
        Ok(()) => ZX_OK,
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_job_policy_query_basic_policy(policy: &JobPolicy, condition: u32) -> u32 {
    policy.query_basic_policy(condition)
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_job_policy_query_basic_policy_override(
    policy: &JobPolicy,
    condition: u32,
) -> u32 {
    policy.query_basic_policy_override(condition)
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_job_policy_set_timer_slack(policy: &mut JobPolicy, slack: &TimerSlack) {
    policy.set_timer_slack(*slack);
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_job_policy_get_timer_slack(policy: &JobPolicy, out_slack: &mut TimerSlack) {
    *out_slack = policy.get_timer_slack();
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_job_policy_eq(lhs: &JobPolicy, rhs: &JobPolicy) -> bool {
    lhs == rhs
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_job_policy_increment_counter(action: u32, condition: u32) {
    JobPolicy::increment_counter(action, condition);
}
