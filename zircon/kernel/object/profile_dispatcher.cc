// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <bits.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/scheduler_state.h>
#include <ktl/bit.h>
#include <object/profile_dispatcher.h>

#include <ktl/enforce.h>

zx::result<SchedulerState::BaseProfile> validate_and_create_profile(const zx_profile_info_t& info) {
  // Ensure that none of the flags outside of the set of valid flags has been set.
  constexpr uint32_t kAllFlags = (ZX_PROFILE_INFO_FLAG_PRIORITY | ZX_PROFILE_INFO_FLAG_CPU_MASK |
                                  ZX_PROFILE_INFO_FLAG_DEADLINE | ZX_PROFILE_INFO_FLAG_NO_INHERIT |
                                  ZX_PROFILE_INFO_FLAG_CRITICAL);
  if ((info.flags & ~kAllFlags) != 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Ensure that exactly one of the "discipline selection" flags has been set.
  constexpr uint32_t kDisciplineSelectionFlags =
      ZX_PROFILE_INFO_FLAG_PRIORITY | ZX_PROFILE_INFO_FLAG_DEADLINE;
  if (ktl::popcount(info.flags & kDisciplineSelectionFlags) != 1) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Deadline profiles may not be flagged as NO_INHERIT
  constexpr uint32_t kDeadlineNoInherit =
      ZX_PROFILE_INFO_FLAG_DEADLINE | ZX_PROFILE_INFO_FLAG_NO_INHERIT;
  if ((info.flags & kDeadlineNoInherit) == kDeadlineNoInherit) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // If selected, ensure priority is valid.
  if ((info.flags & ZX_PROFILE_INFO_FLAG_PRIORITY) != 0) {
    if ((info.priority < LOWEST_PRIORITY) || (info.priority > HIGHEST_PRIORITY)) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  // If selected, ensure the deadline parameters are valid.  Note that deadline
  // profiles must currently be inheritable.
  const bool inheritable = (info.flags & ZX_PROFILE_INFO_FLAG_NO_INHERIT) == 0;
  if ((info.flags & ZX_PROFILE_INFO_FLAG_DEADLINE) != 0) {
    // Check that the deadline parameters have the correct relationship to each
    // other.
    const bool admissible =
        info.deadline_params.capacity > 0 &&
        info.deadline_params.capacity <= info.deadline_params.relative_deadline &&
        info.deadline_params.relative_deadline <= info.deadline_params.period && inheritable;
    if (!admissible) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    // Check that the parameters are within the range of the signed 32bit
    // SchedCompactDuration. This permits more compact storage of profile
    // parameters and provides protection against excessive values.
    if (info.deadline_params.capacity > SchedCompactDuration::Max() ||
        info.deadline_params.relative_deadline > SchedCompactDuration::Max() ||
        info.deadline_params.period > SchedCompactDuration::Max()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
  }

  if ((info.flags & ZX_PROFILE_INFO_FLAG_CRITICAL) != 0 &&
      (info.flags & ZX_PROFILE_INFO_FLAG_DEADLINE) == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (info.flags & ZX_PROFILE_INFO_FLAG_PRIORITY) {
    return zx::ok(SchedulerState::BaseProfile(info.priority, inheritable));
  } else {
    DEBUG_ASSERT(inheritable == true);
    const bool critical = (info.flags & ZX_PROFILE_INFO_FLAG_CRITICAL) != 0;
    return zx::ok(SchedulerState::BaseProfile(info.deadline_params, critical));
  }
}

extern "C" {
void rust_profile_dispatcher_state_init(void* state, void* disp, const zx_profile_info_t* info);
void rust_profile_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_profile_dispatcher_state_get_lock(const void* state);
}

ProfileDispatcher::ProfileDispatcher(const zx_profile_info_t& info) : Dispatcher(0u) {
  DISPATCHER_VERIFY_OFFSET(ProfileDispatcher, kProfileDispatcherStateOffset);
  rust_profile_dispatcher_state_init(&opaque_storage_, this, &info);
}

IMPLEMENT_DISPATCHER_RUST_STATE(ProfileDispatcher, rust_profile_dispatcher_state_get_lock,
                                rust_profile_dispatcher_state_destroy)
