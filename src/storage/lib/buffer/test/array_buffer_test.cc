// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/buffer/array_buffer.h"

#include <fidl/fuchsia.storage.block/cpp/natural_types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>

#include <gtest/gtest.h>

namespace storage {
namespace {

const size_t kCapacity = 3;
const uint32_t kBlockSize = 8192;

TEST(ArrayBufferTest, ConstructEmpty) {
  ArrayBuffer buffer(0, kBlockSize);
  EXPECT_EQ(0u, buffer.capacity());
  EXPECT_EQ(fuchsia_storage_block::kVmoidInvalid, buffer.vmoid());
}

TEST(ArrayBufferTest, ConstructValid) {
  ArrayBuffer buffer(kCapacity, kBlockSize);
  EXPECT_EQ(kCapacity, buffer.capacity());
  EXPECT_EQ(kBlockSize, buffer.BlockSize());
  EXPECT_EQ(fuchsia_storage_block::kVmoidInvalid, buffer.vmoid());
  EXPECT_NE(nullptr, buffer.Data(0));
}

TEST(ArrayBufferTest, WriteToReadFromBuffer) {
  ArrayBuffer buffer(kCapacity, kBlockSize);
  char buf[kBlockSize];
  memset(buf, 'a', sizeof(buf));

  for (size_t i = 0; i < kCapacity; i++) {
    memcpy(buffer.Data(i), buf, kBlockSize);
  }
  for (size_t i = 0; i < kCapacity; i++) {
    EXPECT_EQ(0, memcmp(buf, buffer.Data(i), kBlockSize));
  }
}

}  // namespace
}  // namespace storage
