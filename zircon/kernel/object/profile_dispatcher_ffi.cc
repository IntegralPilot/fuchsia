// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <kernel/scheduler_state.h>
#include <object/handle.h>
#include <object/profile_dispatcher.h>

extern "C" {

zx_status_t cpp_profile_dispatcher_create(
    const zx_profile_info_t* info,
    ffi::Uninitialized<KernelHandle<ProfileDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) ProfileDispatcher(*info));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

zx_status_t cpp_profile_dispatcher_validate_and_create_profile(
    const zx_profile_info_t* info, ffi::Uninitialized<SchedulerState::BaseProfile>* profile_out) {
  zx::result<SchedulerState::BaseProfile> maybe_profile = validate_and_create_profile(*info);
  if (maybe_profile.is_error()) {
    return maybe_profile.error_value();
  }
  profile_out->Initialize(maybe_profile.value());
  return ZX_OK;
}

}  // extern "C"
