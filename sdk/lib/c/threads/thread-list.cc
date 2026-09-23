// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thread-list.h"

namespace LIBC_NAMESPACE_DECL {

Mutex gAllThreadsLock;
Thread* gAllThreads;

}  // namespace LIBC_NAMESPACE_DECL
