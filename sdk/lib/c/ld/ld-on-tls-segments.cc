// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/tls.h>

#include "../threads/thread-storage.h"
#include "../threads/thread.h"
#include "ld-abi.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

void OnTlsSegments(Thread& thread, sanitizer_memory_snapshot_callback_t* callback,
                   void* callback_arg) {
  const std::span thread_block = ThreadStorage::ThreadThreadBlock(thread);
  std::byte* const tp = static_cast<std::byte*>(pthread_to_tp(&thread));
  const size_t tp_offset = tp - thread_block.data();
  auto tls_segments = ld::TlsInitialExecSegments(_ld_abi, thread_block, tp_offset);
  for (const auto& [module, segment] : tls_segments) {
    callback(segment.data(), segment.size_bytes(), callback_arg);
  }
}

}  // namespace LIBC_NAMESPACE_DECL
