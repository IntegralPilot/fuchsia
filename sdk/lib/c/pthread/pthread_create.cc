// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_create.h"

#include "../threads/thread.h"
#include "attr.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_create,
                   (pthread_t* __restrict thread, const pthread_attr_t* __restrict attr,
                    __pthread_start_t func, void* arg)) {
  return ThreadCreateAndStart<ToPthread, PthreadError>(
      thread, FromPthreadAttr(attr).WithDefaultName("pthread_create:%p", func),
      ToThreadFunction(func), arg);
}

}  // namespace LIBC_NAMESPACE_DECL
