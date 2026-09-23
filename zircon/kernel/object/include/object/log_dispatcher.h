// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_LOG_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_LOG_DISPATCHER_H_

#include <lib/object-constants.h>
#include <zircon/rights.h>
#include <zircon/syscalls/log.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class LogDispatcher;
extern "C" {
// Allocates the C++ LogDispatcher instance. Implemented in C++ (log_dispatcher_ffi.cc)
// and called by Rust during LogDispatcher::create.
zx_status_t cpp_log_dispatcher_create(uint32_t flags, zx_rights_t rights,
                                      ffi::Uninitialized<KernelHandle<LogDispatcher>>* handle_out);

// Entry point for C++ code to create a LogDispatcher. Implemented in Rust
// (log_dispatcher_ffi.rs) and called by LogDispatcher::Create. It orchestrates
// the creation by calling Rust's LogDispatcher::create, which in turn calls
// cpp_log_dispatcher_create.
zx_status_t rust_log_dispatcher_create(uint32_t flags, KernelHandle<LogDispatcher>* handle_out,
                                       zx_rights_t* rights_out);
}

class LogDispatcher final : public Dispatcher {
 public:
  // Helper for internal kernel callers (such as userboot.cc) to create a LogDispatcher.
  static zx_status_t Create(uint32_t flags, KernelHandle<LogDispatcher>* handle,
                            zx_rights_t* rights) {
    return rust_log_dispatcher_create(flags, handle, rights);
  }

  ~LogDispatcher() final;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_LOG; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return true; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final;
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  using Dispatcher::UpdateState;
  using Dispatcher::UpdateStateLocked;

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_log_dispatcher_create(uint32_t, zx_rights_t,
                                               ffi::Uninitialized<KernelHandle<LogDispatcher>>*);
  explicit LogDispatcher(uint32_t flags);

  OpaqueStorage<kLogDispatcherStateSize, kLogDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_LOG_DISPATCHER_H_
