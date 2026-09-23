// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_detach.h"

#include "../threads/thread.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_detach, (pthread_t thread)) {
  zx::result result = ThreadDetach(*FromPthread(thread));
  return PthreadError(result.status_value());
}

}  // namespace LIBC_NAMESPACE_DECL
