// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZX_THREAD_H_
#define LIB_ZX_THREAD_H_

#include <lib/zx/object.h>
#include <lib/zx/task.h>
#include <zircon/availability.h>
#include <zircon/process.h>
#include <zircon/syscalls/exception.h>

namespace zx {
class process;

class thread final : public task<thread> {
 public:
  static constexpr zx_obj_type_t TYPE = ZX_OBJ_TYPE_THREAD;

  constexpr thread() = default;

  explicit thread(zx_handle_t value) : task(value) {}

  explicit thread(handle&& h) : task(h.release()) {}

  thread(thread&& other) : task(other.release()) {}

  thread& operator=(thread&& other) {
    reset(other.release());
    return *this;
  }

  // Rather than creating a thread directly with this syscall, consider using
  // std::thread or thrd_create, which properly integrates with the
  // thread-local data structures in libc.
  static zx_status_t create(const process& process, const char* name, uint32_t name_len,
                            uint32_t flags, thread* result) ZX_AVAILABLE_SINCE(7);

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  zx_status_t start(uint64_t pc, uint64_t sp,
                    // PC and SP must be given, but others default to zero.
                    uint64_t arg1 = 0, uint64_t arg2 = 0, uint64_t tp = 0,
                    uint64_t abi_reg = 0) const ZX_AVAILABLE_SINCE(31) {
    return zx_thread_start_regs(get(), pc, sp, arg1, arg2, tp, abi_reg);
  }
#else
  // The first variant maps exactly to the syscall and can be used for
  // launching threads in remote processes. The second variant is for
  // conveniently launching threads in the current process.
  zx_status_t start(uintptr_t thread_entry, uintptr_t stack, uintptr_t arg1, uintptr_t arg2) const
      ZX_AVAILABLE_SINCE(7) {
    return zx_thread_start(get(), thread_entry, stack, arg1, arg2);
  }
  zx_status_t start(void (*thread_entry)(uintptr_t arg1, uintptr_t arg2), void* stack,
                    uintptr_t arg1, uintptr_t arg2) const ZX_AVAILABLE_SINCE(7) {
    return zx_thread_start(get(), reinterpret_cast<uintptr_t>(thread_entry),
                           reinterpret_cast<uintptr_t>(stack), arg1, arg2);
  }
#endif

  zx_status_t read_state(uint32_t kind, void* buffer, size_t len) const ZX_AVAILABLE_SINCE(7) {
    return zx_thread_read_state(get(), kind, buffer, len);
  }
  zx_status_t write_state(uint32_t kind, const void* buffer, size_t len) const
      ZX_AVAILABLE_SINCE(7) {
    return zx_thread_write_state(get(), kind, buffer, len);
  }

  static inline zx_status_t raise_exception(uint32_t options, zx_excp_type_t type,
                                            const zx_exception_context_t* context)
      ZX_AVAILABLE_SINCE(20) {
    return zx_thread_raise_exception(options, type, context);
  }

  static inline unowned<thread> self() ZX_AVAILABLE_SINCE(7) {
    return unowned<thread>(zx_thread_self());
  }
};

using unowned_thread = unowned<thread> ZX_AVAILABLE_SINCE(7);

}  // namespace zx

#endif  // LIB_ZX_THREAD_H_
