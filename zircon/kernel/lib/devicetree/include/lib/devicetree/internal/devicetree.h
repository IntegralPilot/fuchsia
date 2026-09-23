// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_INTERNAL_DEVICETREE_H_
#define ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_INTERNAL_DEVICETREE_H_

#include <inttypes.h>
#include <zircon/assert.h>

#include <cstdint>
#include <span>

namespace devicetree::internal {

using ByteView = std::span<const uint8_t>;

struct ReadBigEndianUint32Result {
  uint32_t value;
  ByteView tail;
};

inline ReadBigEndianUint32Result ReadBigEndianUint32(ByteView bytes) {
  ZX_ASSERT(bytes.size() >= sizeof(uint32_t));
  return {
      (static_cast<uint32_t>(bytes[0]) << 24) | (static_cast<uint32_t>(bytes[1]) << 16) |
          (static_cast<uint32_t>(bytes[2]) << 8) | static_cast<uint32_t>(bytes[3]),
      bytes.subspan(sizeof(uint32_t)),
  };
}

template <size_t N>
constexpr uint64_t ParseCells(ByteView bytes) {
  static_assert(N == 1 || N == 2, "Only supports 1 or 2 cells.");
  uint64_t higher = ReadBigEndianUint32(bytes).value;
  if constexpr (N == 1) {
    return higher;
  } else {
    uint64_t lower = ReadBigEndianUint32(bytes.subspan(4)).value;
    return higher << 32 | lower;
  }
}

constexpr uint64_t ParseCells(ByteView bytes, uint32_t num_cells) {
  switch (num_cells) {
    case 1:
      return ParseCells<1>(bytes);
    case 2:
      return ParseCells<2>(bytes);
    default:
      ZX_PANIC("Invalid number of cells (%" PRIu32 ")", num_cells);
  }
}

}  // namespace devicetree::internal

#endif  // ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_INTERNAL_DEVICETREE_H_
