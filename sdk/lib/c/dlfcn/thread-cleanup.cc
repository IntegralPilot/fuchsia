// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../threads/thread.h"

namespace LIBC_NAMESPACE_DECL {

void ThreadDlfcnCleanup() {
  // TODO(https://fxbug.dev/325494556): dlerror state
}

}  // namespace LIBC_NAMESPACE_DECL
