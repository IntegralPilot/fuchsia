// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "libc.h"
#include "start-main.h"

namespace LIBC_NAMESPACE_DECL {

PageRoundedSize InitialStackSize() {
  // The value was collected or defaulted in dls3 (dynlink.c).
  return *PageRoundedSize::From(_dl_stack_size());
}

}  // namespace LIBC_NAMESPACE_DECL
