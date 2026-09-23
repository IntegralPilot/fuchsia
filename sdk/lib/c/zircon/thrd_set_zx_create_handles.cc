// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/threads.h>

#include <utility>

#include "threads_impl.h"

__EXPORT thrd_zx_create_handles_t thrd_set_zx_create_handles(thrd_zx_create_handles_t handles) {
  return std::exchange(__pthread_self()->create_handles, handles);
}
