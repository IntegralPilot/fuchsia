// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/trace/trace_enabled.h"

#include <zircon/process.h>
#include <zircon/types.h>

#include <atomic>
#include <cstdint>

namespace storage::trace {

namespace {
std::atomic_uint32_t current_trace_id{0u};
}  // namespace

uint64_t GenerateTraceId() {
  static zx_handle_t self_handle = zx_process_self();
  return self_handle + (uint64_t{current_trace_id.fetch_add(1u, std::memory_order_relaxed)} << 32);
}

}  // namespace storage::trace
