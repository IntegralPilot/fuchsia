// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/threads/thrd_exit.h"

#include "src/__support/common.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(void, thrd_exit, (int retval)) { ThreadExit(retval); }

}  // namespace LIBC_NAMESPACE_DECL
