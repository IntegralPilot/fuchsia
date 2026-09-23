// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "libc.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

void ThreadDlfcnCleanup() { __dl_thread_cleanup(); }

}  // namespace LIBC_NAMESPACE_DECL
