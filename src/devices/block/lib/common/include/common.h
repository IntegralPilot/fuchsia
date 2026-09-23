// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_LIB_COMMON_INCLUDE_COMMON_H_
#define SRC_DEVICES_BLOCK_LIB_COMMON_INCLUDE_COMMON_H_

#include <zircon/types.h>

#include "sdk/lib/driver/logging/cpp/logger.h"

namespace block {

// Check whether the IO request fits within the block device.
inline zx_status_t CheckIoRange(uint64_t block_offset, uint32_t transfer_blocks,
                                uint64_t total_block_count, fdf::Logger& logger) {
  if (transfer_blocks == 0 || transfer_blocks > total_block_count) {
    logger.log(fdf::ERROR,
               "IO request length ({} blocks) is zero or exceeds the total block count ({}).",
               transfer_blocks, total_block_count);
    return ZX_ERR_OUT_OF_RANGE;
  }
  if (block_offset >= total_block_count || block_offset > total_block_count - transfer_blocks) {
    logger.log(fdf::ERROR,
               "IO request offset ({} blocks) and length ({} blocks) does not fit within the total "
               "block count ({}).",
               block_offset, transfer_blocks, total_block_count);
    return ZX_ERR_OUT_OF_RANGE;
  }
  return ZX_OK;
}

// Check whether the IO request fits within the block device.
// Also check that the IO request length does not exceed the max transfer size.
inline zx_status_t CheckIoRange(uint64_t block_offset, uint32_t transfer_blocks,
                                uint64_t total_block_count, uint32_t max_transfer_blocks,
                                fdf::Logger& logger) {
  if (transfer_blocks > max_transfer_blocks) {
    logger.log(fdf::ERROR, "IO request length ({} blocks) exceeds max transfer size ({} blocks).",
               transfer_blocks, max_transfer_blocks);
    return ZX_ERR_OUT_OF_RANGE;
  }
  return CheckIoRange(block_offset, transfer_blocks, total_block_count, logger);
}

inline uint32_t ReadFromBigEndian24(const uint8_t* ptr) {
  return ptr[0] << 16 | ptr[1] << 8 | ptr[2];
}

inline zx_status_t WriteToBigEndian24(uint32_t value, uint8_t* ptr) {
  if (value > 0xffffff) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  ptr[0] = (value >> 16) & 0xff;
  ptr[1] = (value >> 8) & 0xff;
  ptr[2] = value & 0xff;
  return ZX_OK;
}

inline uint32_t ReadFromLittleEndian24(const uint8_t* ptr) {
  return ptr[2] << 16 | ptr[1] << 8 | ptr[0];
}

inline zx_status_t WriteToLittleEndian24(uint32_t value, uint8_t* ptr) {
  if (value > 0xffffff) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  ptr[2] = (value >> 16) & 0xff;
  ptr[1] = (value >> 8) & 0xff;
  ptr[0] = value & 0xff;
  return ZX_OK;
}

}  // namespace block

#endif  // SRC_DEVICES_BLOCK_LIB_COMMON_INCLUDE_COMMON_H_
