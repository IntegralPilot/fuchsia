// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_attr_setdetachstate.h"

#include "attr.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_attr_setdetachstate, (pthread_attr_t * attr, int detachstate)) {
  switch (detachstate) {
    case PTHREAD_CREATE_DETACHED:
    case PTHREAD_CREATE_JOINABLE:
      attr->_a_detach = detachstate;
      return 0;

    default:
      return EINVAL;
  }
}

}  // namespace LIBC_NAMESPACE_DECL
