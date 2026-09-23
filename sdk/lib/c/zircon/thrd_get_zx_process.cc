// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/threads.h>

#include "threads_impl.h"

__EXPORT zx_handle_t thrd_get_zx_process() { return __pthread_self()->create_handles.process; }
