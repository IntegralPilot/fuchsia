// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_attr_getstack.h"

#include "attr.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_attr_getstack,
                   (const pthread_attr_t *__restrict attr, void **__restrict stack,
                    size_t *__restrict stacksize)) {
  if (attr->_a_stackaddr == NULL)
    return EINVAL;
  *stack = attr->_a_stackaddr;
  *stacksize = attr->_a_stacksize;
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
