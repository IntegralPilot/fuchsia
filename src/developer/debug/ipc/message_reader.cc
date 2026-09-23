// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/ipc/message_reader.h"

#include <string.h>

#include <cstdint>

namespace debug_ipc {

void MessageReader::SerializeBytes(void* data, uint32_t len) {
  if (has_error_) {
    return;
  }
  if (message_.size() - offset_ < len) {
    has_error_ = true;
  } else {
    memcpy(data, &message_[offset_], len);
    offset_ += len;
  }
}

}  // namespace debug_ipc
