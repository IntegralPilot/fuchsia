// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/stdlib/exit.h"

#include "../weak.h"
#include "libc.h"
#include "src/__support/common.h"
#include "src/stdlib/_Exit.h"
#include "stdio_impl.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(void, exit, (int status)) {
  __tls_run_dtors();
  __funcs_on_exit();
  __libc_exit_fini();
  __stdio_exit();
  Weak<__libc_extensions_fini>::Call();
  _Exit(status);
}

}  // namespace LIBC_NAMESPACE_DECL
