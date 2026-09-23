// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_getname_np.h"

#include <lib/zx/thread.h>
#include <zircon/assert.h>

#include "../zircon/zx-name.h"
#include "src/__support/common.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_getname_np, (pthread_t th, char* buf, size_t len)) {
  pthread* thread = reinterpret_cast<pthread*>(th);
  zx::unowned_thread thread_handle{thread->handle_};
  zx::result result = ZxName::Get(*thread_handle);
  ZX_ASSERT_MSG(result.is_ok(), "Cannot get ZX_PROP_NAME on thread handle: %s",
                result.status_string());

  std::string_view name = result->str();
  if (len <= name.size()) {
    return ERANGE;
  }

  buf[name.copy(buf, len)] = '\0';
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
