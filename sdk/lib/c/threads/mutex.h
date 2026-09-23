// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_MUTEX_H_
#define LIB_C_THREADS_MUTEX_H_

#include <lib/sync/cpp/mutex.h>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// Note this meets the C++ Lockable requirements but is not the same as
// llvm-libc's Mutex from its src/__support/threads/mutex.h header.
//
// That disparity doesn't need to be addressed until any llvm-libc code built
// for Fuchsia uses Mutex.
using libsync::Mutex;

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_THREADS_MUTEX_H_
