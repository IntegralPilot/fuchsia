// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PROFILE_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PROFILE_DISPATCHER_H_

#include <lib/object-constants.h>
#include <zircon/rights.h>
#include <zircon/syscalls/profile.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <kernel/scheduler_state.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

extern "C" {
zx_status_t cpp_profile_dispatcher_create(
    const zx_profile_info_t* info, ffi::Uninitialized<KernelHandle<ProfileDispatcher>>* handle_out);
zx_status_t cpp_profile_dispatcher_validate_and_create_profile(
    const zx_profile_info_t* info, ffi::Uninitialized<SchedulerState::BaseProfile>* profile_out);
}

zx::result<SchedulerState::BaseProfile> validate_and_create_profile(const zx_profile_info_t& info);

class ProfileDispatcher final : public Dispatcher {
 public:
  explicit ProfileDispatcher(const zx_profile_info_t& info);
  ~ProfileDispatcher() final;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_PROFILE; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return false; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  OpaqueStorage<kProfileDispatcherStateSize, kProfileDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PROFILE_DISPATCHER_H_
