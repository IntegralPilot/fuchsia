// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_exit.h"

#include "../threads/thread.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(void, pthread_exit, (void *retval)) {
  ThreadExit(reinterpret_cast<intptr_t>(retval));
}

}  // namespace LIBC_NAMESPACE_DECL
