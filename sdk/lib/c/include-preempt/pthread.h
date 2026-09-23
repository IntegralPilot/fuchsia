// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_PTHREAD_H_
#define PREEMPT_PTHREAD_H_

// The llvm-libc public header provides this and the llvm-libc internal
// headers expect that it will.
#include "include/llvm-libc-types/__pthread_start_t.h"  // IWYU pragma: export

// After that, just wrap the public (legacy implementation of) <pthread.h>.
#include_next <pthread.h>  // IWYU pragma: export

#endif  // PREEMPT_PTHREAD_H_
