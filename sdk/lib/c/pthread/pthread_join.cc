// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_join.h"

#include <cerrno>

#include "../threads/thread.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_join, (pthread_t th, void** retval)) {
  Thread& thread = *FromPthread(th);
  zx::result result = ThreadJoin(thread);
  if (result.is_error()) {
    return EINVAL;
  }
  if (retval) {
    *retval = reinterpret_cast<void*>(result.value());
  }
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
