// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/stdlib/abort.h"

#include <lib/zircon-internal/unique-backtrace.h>

#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(void, abort, ()) { CRASH_WITH_UNIQUE_BACKTRACE(); }

}  // namespace LIBC_NAMESPACE_DECL
