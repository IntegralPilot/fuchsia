// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fxt_writer.h"

#include <lib/syslog/cpp/macros.h>

#include <src/lib/fsl/socket/strings.h>

namespace profiler {
void profiler::FxtRecordBuffer::WriteBytes(const void* buffer, size_t num_bytes) {
  // Calculate how many 64-bit words are needed for the given number of bytes.
  size_t num_words = (num_bytes + sizeof(uint64_t) - 1) / sizeof(uint64_t);

  size_t old_size_in_words = buffer_.size();
  buffer_.resize(old_size_in_words + num_words);
  uint64_t* dest = buffer_.data() + old_size_in_words;
  dest[num_words - 1] = 0;
  memcpy(dest, buffer, num_bytes);
}

void profiler::FxtRecordBuffer::Commit() {
  if (!fsl::BlockingCopyFromString(std::string_view(reinterpret_cast<const char*>(buffer_.data()),
                                                    buffer_.size() * sizeof(uint64_t)),
                                   socket_)) {
    FX_LOGS(ERROR) << "Failed to write samples to socket";
  }
}

zx::result<profiler::FxtRecordBuffer> profiler::FxtWriter::Reserve(uint64_t header) {
  return zx::ok(profiler::FxtRecordBuffer(socket_, header));
}

}  // namespace profiler
