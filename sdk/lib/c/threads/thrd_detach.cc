// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/threads/thrd_detach.h"

#include "src/__support/common.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, thrd_detach, (thrd_t thread)) {
  zx::result result = ThreadDetach(*FromC11Thread(thread));
  return result.is_error() ? thrd_error : thrd_success;
}

}  // namespace LIBC_NAMESPACE_DECL
