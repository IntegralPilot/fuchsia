// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>
#include <zircon/sanitizer.h>

#include "log.h"

__EXPORT void __sanitizer_log_write(const char* buffer, size_t len) {
  LIBC_NAMESPACE::gLog.SymbolizerContext();
  LIBC_NAMESPACE::gLog({buffer, len});
}
