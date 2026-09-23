// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <ramdevice-client/ramdisk.h>

#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace block_client {

// Tests that the RemoteBlockDevice is capable of transmitting and receiving
// messages with the block device.
TEST(BlockClientRamdiskTest, WriteReadBlock) {
  constexpr int kBlockSize = 4096;
  auto ram_disk = ramdevice_client::Ramdisk::Create(kBlockSize, 10);
  ASSERT_EQ(ram_disk.status_value(), ZX_OK);

  auto block = ram_disk->ConnectBlock();
  ASSERT_EQ(block.status_value(), ZX_OK);

  constexpr size_t max_count = 3;

  std::vector<uint8_t> write_buffer(kBlockSize * max_count + 5),
      read_buffer(kBlockSize * max_count);
  // write some pattern to the write buffer
  for (size_t i = 0; i < kBlockSize * max_count; ++i) {
    write_buffer[i] = static_cast<uint8_t>(i % 251);
  }
  // Test that unaligned counts and offsets result in failures:
  ASSERT_NE(SingleWriteBytes(*block, write_buffer.data(), 5, 0), ZX_OK);
  ASSERT_NE(SingleWriteBytes(*block, write_buffer.data(), kBlockSize, 5), ZX_OK);
  ASSERT_NE(SingleWriteBytes(*block, nullptr, kBlockSize, 0), ZX_OK);
  ASSERT_NE(SingleReadBytes(*block, read_buffer.data(), 5, 0), ZX_OK);
  ASSERT_NE(SingleReadBytes(*block, read_buffer.data(), kBlockSize, 5), ZX_OK);
  ASSERT_NE(SingleReadBytes(*block, nullptr, kBlockSize, 0), ZX_OK);

  // test multiple counts, multiple offsets
  for (uint64_t count = 1; count < max_count; ++count) {
    for (uint64_t offset = 0; offset < 2; ++offset) {
      size_t buffer_offset = count + 10 * offset;
      ASSERT_EQ(SingleWriteBytes(*block, write_buffer.data() + buffer_offset, kBlockSize * count,
                                 kBlockSize * offset),
                ZX_OK);
      ASSERT_EQ(
          SingleReadBytes(*block, read_buffer.data(), kBlockSize * count, kBlockSize * offset),
          ZX_OK);
      ASSERT_EQ(memcmp(write_buffer.data() + buffer_offset, read_buffer.data(), kBlockSize * count),
                0);
    }
  }
}

}  // namespace block_client
