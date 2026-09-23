// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_KERNEL_VDSO_VDSO_COMMON_H_
#define SRC_STARNIX_KERNEL_VDSO_VDSO_COMMON_H_

#include <sys/time.h>

// The platform agnostic VDSO implementations.

// An implementation of the clock_gettime syscall.
int clock_gettime_impl(int clock_id, timespec* tp);

// An implementation of the clock_getres syscall.
int clock_getres_impl(int clock_id, timespec* tp);

// An implementation of the gettimeofday syscall.
int gettimeofday_impl(timeval* tv, struct timezone* tz);

#endif  // SRC_STARNIX_KERNEL_VDSO_VDSO_COMMON_H_
