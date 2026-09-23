// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstddef>
#include <cstdint>
#include <span>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

size_t BacktraceByFramePointer(std::span<uintptr_t> pcs);

size_t BacktraceByShadowCallStack(std::span<uintptr_t> pcs);

#if !__has_feature(shadow_call_stack)
inline size_t BacktraceByShadowCallStack(std::span<uintptr_t> pcs) { return 0; }
#endif

}  // namespace LIBC_NAMESPACE_DECL
