// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_attr_setstacksize.h"

#include <cstdint>

#include "attr.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_attr_setstacksize,
                   (pthread_attr_t *__restrict attr, size_t stacksize)) {
  if (stacksize < PTHREAD_STACK_MIN ||               // Detect too-small size
      stacksize - PTHREAD_STACK_MIN > SIZE_MAX / 4)  // ... or overflow risk.
    return EINVAL;
  attr->_a_stacksize = stacksize;
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
