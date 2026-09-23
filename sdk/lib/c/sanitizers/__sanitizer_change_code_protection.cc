// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/sanitizer.h>

__EXPORT zx_status_t __sanitizer_change_code_protection(uintptr_t addr, size_t len, bool writable) {
  return ZX_ERR_NOT_SUPPORTED;
}
