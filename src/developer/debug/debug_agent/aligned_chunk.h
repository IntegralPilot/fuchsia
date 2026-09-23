// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ALIGNED_CHUNK_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ALIGNED_CHUNK_H_

#include <lib/fit/function.h>
#include <stdint.h>

namespace debug_agent {

// Converts writing an array to a sequence of full-64-bit-word reads and writes (for ptrace memory
// functions). This is provided as a generic function so it can easily be unit testable on top of
// simpler platform APIs.
bool WriteAligned64Chunks(const void* buf, size_t len,
                          fit::function<bool(uint64_t addr, uint64_t* value)> read,
                          fit::function<bool(uint64_t addr, uint64_t value)> write,
                          uint64_t dest_addr);

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ALIGNED_CHUNK_H_
