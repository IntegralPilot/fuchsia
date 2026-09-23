// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_HOST_ENCODER_H_
#define LIB_SYSLOG_CPP_HOST_ENCODER_H_

#include <assert.h>
#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/macros.h>

#include <cstring>

// This file contains shared implementations for writing string logs between the legacy backend and
// the host backend

namespace fuchsia_logging::internal {
struct MsgHeader {
  fuchsia_logging::RawLogSeverity severity;
  char* offset;
  LogBuffer* buffer;
  bool first_tag;
  char* user_tag;
  bool has_msg;
  bool first_kv;
  bool terminated;
  size_t RemainingSpace() {
    auto end = (reinterpret_cast<const char*>(this) + sizeof(LogBuffer));
    auto avail = static_cast<size_t>(end - offset -
                                     2);  // Allocate an extra byte for a NULL terminator at the end
    return avail;
  }

  void NullTerminate() {
    if (!terminated) {
      terminated = true;
      *(offset++) = 0;
    }
  }

  const char* c_str() {
    NullTerminate();
    return reinterpret_cast<const char*>(buffer->data());
  }

  void WriteChar(const char value) {
    if (!RemainingSpace()) {
      [[maybe_unused]] zx::result<> result = FlushToGlobalLogger(*buffer);
      offset = reinterpret_cast<char*>(buffer->data());
      WriteString("CONTINUATION: ");
    }
    assert((offset + 1) < (reinterpret_cast<const char*>(this) + sizeof(LogBuffer)));
    *offset = value;
    offset++;
  }

  void WriteString(std::string_view value) {
    size_t total_chars = value.size();
    size_t written_chars = 0;
    while (written_chars < total_chars) {
      size_t written = WriteStringInternal(
          std::string_view(value.data() + written_chars, value.size() - written_chars));
      written_chars += written;
      if (written_chars < total_chars) {
        FlushAndReset();
        WriteStringInternal("CONTINUATION: ");
      }
    }
  }

  void FlushAndReset() {
    [[maybe_unused]] zx::result<> result = FlushToGlobalLogger(*buffer);
    offset = reinterpret_cast<char*>(buffer->data());
  }

  // Writes a string to the buffer and returns the
  // number of bytes written. Returns 0 only if
  // the length of the string is 0, or if we're
  // exactly at the end of the buffer and need a reset.
  size_t WriteStringInternal(std::string_view value) {
    size_t len = value.size();
    auto remaining = RemainingSpace();
    if (len > remaining) {
      len = remaining;
    }
    assert((offset + len) < (reinterpret_cast<const char*>(this) + sizeof(LogBuffer)));
    memcpy(offset, value.data(), len);
    offset += len;
    return len;
  }

  void Init(LogBuffer* log_buffer, fuchsia_logging::RawLogSeverity log_severity) {
    this->severity = log_severity;
    user_tag = nullptr;
    offset = reinterpret_cast<char*>(log_buffer->data());
    first_tag = true;
    has_msg = false;
    first_kv = true;
    terminated = false;
  }

  static MsgHeader* CreatePtr(LogBuffer* log_buffer) {
    return reinterpret_cast<MsgHeader*>(log_buffer->record_state());
  }
};

#ifndef __Fuchsia__
const std::string GetNameForLogSeverity(fuchsia_logging::RawLogSeverity severity);
#endif

static_assert(sizeof(MsgHeader) <= LogBuffer::record_state_size(),
              "message header must be no larger than record_state");
}  // namespace fuchsia_logging::internal

#endif  // LIB_SYSLOG_CPP_HOST_ENCODER_H_
