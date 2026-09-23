// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/ipc/message_writer.h"

#include <string.h>

#include <cstdint>

namespace debug_ipc {

void MessageWriter::SerializeBytes(void* data, uint32_t len) {
  const char* begin = static_cast<const char*>(data);
  const char* end = begin + len;
  buffer_.insert(buffer_.end(), begin, end);
}

std::vector<char> MessageWriter::MessageComplete() {
  uint32_t size = static_cast<uint32_t>(buffer_.size());
  memcpy(buffer_.data(), &size, sizeof(uint32_t));
  return std::move(buffer_);
}

}  // namespace debug_ipc
