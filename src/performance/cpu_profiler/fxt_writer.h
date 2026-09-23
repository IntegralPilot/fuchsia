// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_PERFORMANCE_CPU_PROFILER_FXT_WRITER_H_
#define SRC_PERFORMANCE_CPU_PROFILER_FXT_WRITER_H_

#include <lib/zx/result.h>

#include <cstdint>
#include <vector>

#include "lib/zx/socket.h"

namespace profiler {

class FxtRecordBuffer {
 public:
  explicit FxtRecordBuffer(const zx::socket& socket, uint64_t header)
      : buffer_({header}), socket_(socket) {}
  std::vector<uint64_t> buffer_;
  const zx::socket& socket_;
  void WriteWord(uint64_t word) { buffer_.push_back(word); }
  void WriteBytes(const void* buffer, size_t num_bytes);
  void Commit();
};

class FxtWriter {
 public:
  explicit FxtWriter(zx::socket socket) : socket_(std::move(socket)) {}
  zx::result<FxtRecordBuffer> Reserve(uint64_t header);
  zx::socket socket_;
};
}  // namespace profiler
#endif  // SRC_PERFORMANCE_CPU_PROFILER_FXT_WRITER_H_
