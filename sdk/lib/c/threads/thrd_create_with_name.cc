// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/threads/thrd_create_with_name.h"

#include "src/__support/common.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, thrd_create_with_name,
                   (thrd_t * thread, thrd_start_t func, void* arg, const char* name)) {
  ThreadAttributes attrs;
  if (name) [[likely]] {
    attrs.name = ZxName{name};
  }
  return ThreadCreateAndStart<ToC11Thread, C11ThreadError>(
      thread, attrs.WithDefaultName("thrd_create_with_name:%p", func), ToThreadFunction(func), arg);
}

}  // namespace LIBC_NAMESPACE_DECL
