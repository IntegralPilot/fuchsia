// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zx-name.h"

#include <cstdarg>
#include <cstdio>

namespace LIBC_NAMESPACE_DECL {

ZxName ZxName::Printf(const char* fmt, ...) {
  ZxName result;
  va_list args;
  va_start(args, fmt);
  vsnprintf(result.name_.data(), result.name_.size(), fmt, args);
  va_end(args);
  return result;
}

}  // namespace LIBC_NAMESPACE_DECL
