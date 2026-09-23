// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/pthread/pthread_setname_np.h"

#include <lib/zx/thread.h>
#include <zircon/assert.h>

#include "../zircon/zx-name.h"
#include "src/__support/common.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, pthread_setname_np, (pthread_t th, const char* name)) {
  pthread* thread = reinterpret_cast<pthread*>(th);
  ZxName thread_name{name};
  zx::unowned_thread thread_handle{thread->handle_};
  zx::result result = thread_name.Set(*thread_handle);
  if (result.is_error()) {
    // ZX_ERR_BAD_STATE just means the thread has exited.  It's still valid to
    // try to set its name (if the pthread_t is still valid at all--i.e. it's
    // joinable and not detached), but it's no longer possible.  No other error
    // should be possible without internal corruption like the handle being bad.
    ZX_ASSERT_MSG(result.error_value() == ZX_ERR_BAD_STATE,
                  "Cannot set ZX_PROP_NAME on thread handle: %s", result.status_string());
    return ESRCH;
  }
  return 0;
}

}  // namespace LIBC_NAMESPACE_DECL
