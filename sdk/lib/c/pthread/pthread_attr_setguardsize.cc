// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_attr_setguardsize.h"

#include "attr.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_attr_setguardsize,
                   (pthread_attr_t *__restrict attr, size_t guardsize)) {
  attr->_a_guardsize = guardsize;
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
