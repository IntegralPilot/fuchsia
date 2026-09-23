// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// IWYU pragma: private, include "src/storage/lib/trace/trace.h"

#ifndef SRC_STORAGE_LIB_TRACE_TRACE_ENABLED_H_
#define SRC_STORAGE_LIB_TRACE_TRACE_ENABLED_H_

#include <lib/trace/event.h>  // IWYU pragma: export

#include <cstdint>

namespace storage::trace {

// Generates a trace ID that will be unique across the system (barring overflow of the per-process
// nonce, reuse of a zx_handle_t for two processes, or some other code in this process which uses
// the same procedure to generate IDs).
//
// We use this instead of the standard TRACE_NONCE because TRACE_NONCE is only unique within a
// process; we need IDs that are unique across all processes.
uint64_t GenerateTraceId();

}  // namespace storage::trace

#endif  // SRC_STORAGE_LIB_TRACE_TRACE_ENABLED_H_
