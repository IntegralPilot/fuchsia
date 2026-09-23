// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/dlfcn/dladdr.h"

#include "dladdr-impl.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, dladdr, (const void* __restrict addr, Dl_info* __restrict info)) {
  DladdrResult result = DladdrImpl(addr, info);
  return result.module ? 1 : 0;
}

}  // namespace LIBC_NAMESPACE_DECL
