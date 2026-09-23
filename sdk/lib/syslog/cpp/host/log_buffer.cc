// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/host/encoder.h>
#include <lib/syslog/cpp/host/log_buffer.h>

#include <cinttypes>
#include <iostream>

namespace fuchsia_logging {
namespace {

// Common initialization for all KV pairs.
// Returns the header for writing the value.
internal::MsgHeader* StartKv(LogBuffer* buffer, std::string_view key) {
  auto header = internal::MsgHeader::CreatePtr(buffer);
  if (!header->first_kv || header->has_msg) {
    header->WriteChar(' ');
  }
  header->WriteString(key);
  header->WriteChar('=');
  header->first_kv = false;
  return header;
}

void WriteKeyValueLegacy(LogBuffer* buffer, std::string_view key, std::string_view value) {
  // "tag" has special meaning to our logging API
  if (key == "tag") {
    auto header = internal::MsgHeader::CreatePtr(buffer);
    auto tag_size = value.size() + 1;
    header->user_tag = reinterpret_cast<char*>(buffer->data()) + buffer->data_size() - tag_size;
    memcpy(header->user_tag, value.data(), value.size());
    header->user_tag[value.size()] = '\0';
    return;
  }
  auto header = StartKv(buffer, key);
  header->WriteChar('"');
  if (memchr(value.data(), '"', value.size()) != nullptr) {
    // Escape quotes in strings.
    for (char c : value) {
      if (c == '"') {
        header->WriteChar('\\');
      }
      header->WriteChar(c);
    }
  } else {
    header->WriteString(value);
  }
  header->WriteChar('"');
}

void WriteKeyValueLegacy(LogBuffer* buffer, std::string_view key, int64_t value) {
  auto header = StartKv(buffer, key);
  char a_buffer[128];
  snprintf(a_buffer, 128, "%" PRId64, value);
  header->WriteString(a_buffer);
}

void WriteKeyValueLegacy(LogBuffer* buffer, std::string_view key, uint64_t value) {
  auto header = StartKv(buffer, key);
  char a_buffer[128];
  snprintf(a_buffer, 128, "%" PRIu64, value);
  header->WriteString(a_buffer);
}

void WriteKeyValueLegacy(LogBuffer* buffer, std::string_view key, double value) {
  auto header = StartKv(buffer, key);
  char a_buffer[128];
  snprintf(a_buffer, 128, "%f", value);
  header->WriteString(a_buffer);
}

void WriteKeyValueLegacy(LogBuffer* buffer, std::string_view key, bool value) {
  auto header = StartKv(buffer, key);
  header->WriteString(value ? "true" : "false");
}

}  // namespace

void LogBuffer::WriteKeyValue(std::string_view key, std::string_view value) {
  WriteKeyValueLegacy(this, key, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, int64_t value) {
  WriteKeyValueLegacy(this, key, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, uint64_t value) {
  WriteKeyValueLegacy(this, key, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, double value) {
  WriteKeyValueLegacy(this, key, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, bool value) {
  WriteKeyValueLegacy(this, key, value);
}

}  // namespace fuchsia_logging
