// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_getattr_np.h"

#include "../threads/thread-storage.h"
#include "../threads/thread.h"
#include "src/__support/common.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_getattr_np, (pthread_t th, pthread_attr_t* attr)) {
  __pthread* t = reinterpret_cast<__pthread*>(th);

  const std::span stack = ThreadStorage::ThreadMachineStack(*t);
  const bool detached =
      t->lifecycle_.load(std::memory_order_acquire) == Thread::Lifecycle::DETACHED;

  *attr = {
      ._a_stacksize = stack.size_bytes(),
      ._a_stackaddr = stack.data(),
      ._a_detach = detached,
  };
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
