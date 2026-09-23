// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zircon-internal/default_stack_size.h>

#include "../startup/start-main.h"
#include "ld-abi.h"

namespace LIBC_NAMESPACE_DECL {

PageRoundedSize InitialStackSize() {
  if (_ld_abi.stack_size == 0) {
    return *PageRoundedSize::From(ZIRCON_DEFAULT_STACK_SIZE);
  }
  return *PageRoundedSize::From(_ld_abi.stack_size);
}

}  // namespace LIBC_NAMESPACE_DECL
