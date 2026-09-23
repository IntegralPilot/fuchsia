// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC_THREADS_THRD_CREATE_WITH_NAME_H_
#define PREEMPT_SRC_THREADS_THRD_CREATE_WITH_NAME_H_

#include <threads.h>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

int thrd_create_with_name(thrd_t* thread, thrd_start_t func, void* arg, const char* name);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // PREEMPT_SRC_THREADS_THRD_CREATE_WITH_NAME_H_
