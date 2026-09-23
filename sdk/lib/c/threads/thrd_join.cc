// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/threads/thrd_join.h"

#include "src/__support/common.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, thrd_join, (thrd_t th, int *retval)) {
  zx::result result = ThreadJoin(*FromC11Thread(th));
  if (result.is_error()) {
    return thrd_error;
  }
  if (retval) {
    *retval = static_cast<int>(result.value());
  }
  return thrd_success;
}

}  // namespace LIBC_NAMESPACE_DECL
