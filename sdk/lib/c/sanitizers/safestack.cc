// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include "../threads/thread-storage.h"
#include "threads_impl.h"

// The compiler supports __builtin_* names that just call these.
// There are no public declarations for them.
extern "C" {
void* __get_unsafe_stack_start();  // NOLINT(bugprone-reserved-identifier)
void* __get_unsafe_stack_top();    // NOLINT(bugprone-reserved-identifier)
void* __get_unsafe_stack_ptr();    // NOLINT(bugprone-reserved-identifier)
}  // extern "C"

namespace LIBC_NAMESPACE_DECL {

extern "C" __EXPORT void* __get_unsafe_stack_start() {
  return ThreadStorage::ThreadUnsafeStack(*__pthread_self()).data();
}

extern "C" __EXPORT void* __get_unsafe_stack_top() {
  std::span stack = ThreadStorage::ThreadUnsafeStack(*__pthread_self());
  if (stack.empty()) {
    return nullptr;
  }
  return stack.data() + stack.size();
}

extern "C" __EXPORT void* __get_unsafe_stack_ptr() {
  if constexpr (kSafeStackAbi) {
    return reinterpret_cast<void*>(__pthread_self()->abi.unsafe_sp);
  }
  return nullptr;
}

}  // namespace LIBC_NAMESPACE_DECL
