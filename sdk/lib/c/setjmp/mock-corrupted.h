// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_SETJMP_MOCK_CORRUPTED_H_
#define LIB_C_SETJMP_MOCK_CORRUPTED_H_

#include <lib/fit/function.h>
#include <setjmp.h>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

extern fit::function<void(jmp_buf)> gMockLongjmpCorrupted;

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_SETJMP_MOCK_CORRUPTED_H_
