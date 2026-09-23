// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/dwarf/encoding.h>

#include <bit>

namespace elfldltl::dwarf {

std::optional<Uleb128> Uleb128::Read(std::span<const std::byte> bytes) {
  Uleb128 result;
  unsigned int shift = 0;
  while (!bytes.empty() && shift < 64) [[likely]] {
    ++result.size_bytes;
    const uint8_t byte = static_cast<uint8_t>(bytes.front());
    result.value |= static_cast<uint64_t>(byte & 0x7f) << shift;
    if ((byte & 0x80) == 0) {
      return result;
    }
    bytes = bytes.subspan(1);
    shift += 7;
  }
  return std::nullopt;
}

std::optional<Sleb128> Sleb128::Read(std::span<const std::byte> bytes) {
  Sleb128 result;
  unsigned int shift = 0;
  while (!bytes.empty() && shift < 64) [[likely]] {
    ++result.size_bytes;
    const uint8_t byte = static_cast<uint8_t>(bytes.front());
    result.value |= std::bit_cast<int64_t>(static_cast<uint64_t>(byte & 0x7f) << shift);
    if ((byte & 0x80) == 0) {
      // Shift up and then back down to sign-extend.
      shift = 64 - (shift + 7);
      result.value = (result.value << shift) >> shift;
      return result;
    }
    bytes = bytes.subspan(1);
    shift += 7;
  }
  return std::nullopt;
}

}  // namespace elfldltl::dwarf
