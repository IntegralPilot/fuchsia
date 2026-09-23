// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thread.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

void OnTlsSegments(Thread& thread, sanitizer_memory_snapshot_callback_t* callback,
                   void* callback_arg) {
  const size_t gen = reinterpret_cast<size_t>(thread.head.dtv[0]);
  size_t modid = 0;
  for (auto* mod = _dl_tls_layout().tls_head; mod && ++modid <= gen; mod = mod->next) {
    callback(thread.head.dtv[modid], mod->size, callback_arg);
  }
}

}  // namespace LIBC_NAMESPACE_DECL
