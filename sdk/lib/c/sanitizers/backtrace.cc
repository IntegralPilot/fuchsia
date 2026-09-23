// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "backtrace.h"

#include <lib/arch/backtrace.h>

#include "../threads/thread-storage.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

size_t BacktraceByFramePointer(std::span<uintptr_t> pcs) {
  struct IsOnStack {
    bool operator()(const arch::CallFrame* fp) const {
      std::span stack = ThreadStorage::ThreadMachineStack(*__pthread_self());
      if (stack.size_bytes() < sizeof(*fp)) [[unlikely]] {
        // This should be impossible, but assume nothing in a critical
        // error-reporting path since this might be used after clobberation.
        return false;
      }
      const uintptr_t base = reinterpret_cast<uintptr_t>(stack.data());
      const uintptr_t frame = reinterpret_cast<uintptr_t>(fp);
      return frame >= base && frame - base <= stack.size_bytes() - sizeof(*fp);
    }
  };
  using FpBacktrace = arch::FramePointerBacktrace<IsOnStack>;

  return arch::StoreBacktrace(FpBacktrace::BackTrace(), pcs, __builtin_return_address(0));
}

#if __has_feature(shadow_call_stack)

size_t BacktraceByShadowCallStack(std::span<uintptr_t> pcs) {
  return arch::StoreBacktrace(
      arch::ShadowCallStackBacktrace{
          ThreadStorage::ThreadShadowCallstack(*__pthread_self()),
          arch::GetShadowCallStackPointer(),
      },
      pcs, __builtin_return_address(0));
}

#endif  // __has_feature(shadow_call_stack)

}  // namespace LIBC_NAMESPACE_DECL
