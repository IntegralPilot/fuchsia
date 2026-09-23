// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_attr_getstacksize.h"

#include "attr.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_attr_getstacksize,
                   (const pthread_attr_t *__restrict attr, size_t *__restrict stacksize)) {
  *stacksize = attr->_a_stacksize;
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
