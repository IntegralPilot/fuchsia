// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/threads/thrd_create.h"

#include "src/__support/common.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, thrd_create, (thrd_t * thread, thrd_start_t func, void* arg)) {
  return ThreadCreateAndStart<ToC11Thread, C11ThreadError>(
      thread,
      ThreadAttributes{
          .name = ZxName::Printf("thrd_create:%p", func),
      },
      ToThreadFunction(func), arg);
}

}  // namespace LIBC_NAMESPACE_DECL
