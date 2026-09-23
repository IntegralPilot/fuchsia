// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdio.h>

#include <cinttypes>

#include <fbl/string_printf.h>
#include <zxtest/base/message.h>

namespace zxtest {

Message::Message(std::string_view desc, const SourceLocation& location)
    : text_(desc), location_(location) {}

Message::Message(Message&& other) noexcept = default;
Message::~Message() = default;

namespace internal {

std::string ToHex(const void* ptr, size_t size) {
  if (size == 0) {
    return "<empty>";
  }

  if (ptr == nullptr) {
    return "<nullptr>";
  }

  // 2 char for 2 4 bit pairs in each byte.
  // 1 char for each space after a byte, except for the last byte.
  const size_t expected_size = 3 * size - 1;
  char buffer[expected_size];
  static constexpr char kHexTable[] = {'0', '1', '2', '3', '4', '5', '6', '7',
                                       '8', '9', 'A', 'B', 'C', 'D', 'E', 'F'};
  const uint8_t* cur = static_cast<const uint8_t*>(ptr);
  const uint8_t* end = static_cast<const uint8_t*>(ptr) + size;
  size_t index = 0;
  while (cur != end) {
    buffer[index++] = kHexTable[*cur >> 4];
    buffer[index++] = kHexTable[*cur & 0xF];
    if (end - cur > 1) {
      buffer[index++] = ' ';
    }
    ++cur;
  }

  return std::string(buffer, index);
}

std::string_view PrintVolatile(volatile const void* ptr, size_t size) {
  if (size == 0) {
    return "<empty>";
  }

  if (ptr == nullptr) {
    return "<nullptr>";
  }

  return "<ptr>";
}

}  // namespace internal

std::string PrintValue(uint32_t value) { return std::string{fbl::StringPrintf("%" PRIu32, value)}; }

std::string PrintValue(int32_t value) { return std::string{fbl::StringPrintf("%" PRIi32, value)}; }

std::string PrintValue(int64_t value) { return std::string{fbl::StringPrintf("%" PRIi64, value)}; }

std::string PrintValue(uint64_t value) { return std::string{fbl::StringPrintf("%" PRIu64, value)}; }

std::string PrintValue(float value) { return std::string{fbl::StringPrintf("%f", value)}; }

std::string PrintValue(double value) { return std::string{fbl::StringPrintf("%f", value)}; }

std::string PrintValue(const char* value) {
  if (value == nullptr) {
    return "<nullptr>";
  }
  return std::string{value};
}

std::string PrintStatus(zx_status_t status) {
#ifdef __Fuchsia__
  return std::string{fbl::StringPrintf("%s(%d)", zx_status_get_string(status), status)};
#else
  return std::to_string(status);
#endif
}

}  // namespace zxtest
