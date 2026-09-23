// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/memory_dump.h"

#include <limits>

#include <gtest/gtest.h>

#include "src/lib/unwinder/error.h"

namespace zxdb {

TEST(MemoryDump, Empty) {
  MemoryDump empty;
  EXPECT_EQ(0u, empty.address());
  EXPECT_EQ(0u, empty.size());

  uint8_t byte = 65;
  EXPECT_FALSE(empty.GetByte(0u, &byte));
  EXPECT_EQ(0, byte);

  byte = 65;
  EXPECT_FALSE(empty.GetByte(0x1234556, &byte));
  EXPECT_EQ(0, byte);
}

TEST(MemoryDump, Valid) {
  std::vector<debug_ipc::MemoryBlock> input;
  input.resize(3);

  uint64_t begin1 = 0x1000;
  uint64_t begin2 = 0x2000;
  uint64_t begin3 = 0x3000;
  uint64_t end = 0x4000;

  // Invalid block.
  input[0].address = begin1;
  input[0].size = begin2 - begin1;
  input[0].valid = false;

  // Valid block filled with cycling bytes.
  input[1].address = begin2;
  input[1].size = begin3 - begin2;
  input[1].valid = true;
  for (uint64_t i = 0; i < input[1].size; i++) {
    input[1].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  // Invalid block.
  input[2].address = begin3;
  input[2].size = end - begin3;
  input[2].valid = false;

  MemoryDump dump(std::move(input));

  // Read from before begin.
  uint8_t byte;
  EXPECT_FALSE(dump.GetByte(0x100, &byte));
  EXPECT_EQ(0u, byte);

  // Read from first invalid block.
  EXPECT_FALSE(dump.GetByte(begin1, &byte));
  EXPECT_FALSE(dump.GetByte(begin1 + 10, &byte));
  EXPECT_FALSE(dump.GetByte(begin2 - 1, &byte));

  // Read from valid block.
  EXPECT_TRUE(dump.GetByte(begin2, &byte));
  EXPECT_EQ(0u, byte);
  EXPECT_TRUE(dump.GetByte(begin2 + 10, &byte));
  EXPECT_EQ(10u, byte);
  EXPECT_TRUE(dump.GetByte(begin3 - 1, &byte));
  EXPECT_EQ((begin3 - 1) % 0x100, byte);

  // Read from third invalid block.
  EXPECT_FALSE(dump.GetByte(begin3, &byte));
  EXPECT_FALSE(dump.GetByte(begin3 + 10, &byte));
  EXPECT_FALSE(dump.GetByte(end - 1, &byte));

  // Read from past the end.
  EXPECT_FALSE(dump.GetByte(end, &byte));
  EXPECT_FALSE(dump.GetByte(end + 1000, &byte));
}

TEST(MemoryDump, Limits) {
  uint64_t max = std::numeric_limits<uint64_t>::max();

  std::vector<debug_ipc::MemoryBlock> block;
  block.resize(1);
  block[0].size = 0x1000;
  block[0].address = max - block[0].size + 1;
  block[0].valid = true;
  for (uint64_t i = 0; i < block[0].size; i++) {
    block[0].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  MemoryDump dump(std::move(block));

  // Query last byte.
  uint8_t byte = 10;
  EXPECT_TRUE(dump.GetByte(max, &byte));
  EXPECT_EQ(max % 0x100, byte);
}

TEST(MemoryDump, ReadBytes) {
  std::vector<debug_ipc::MemoryBlock> input;
  input.resize(3);

  uint64_t begin1 = 0x1000;
  uint64_t begin2 = 0x2000;
  uint64_t begin3 = 0x3000;
  uint64_t end = 0x4000;

  // Valid blocks filled with cycling bytes.
  input[0].address = begin1;
  input[0].size = begin2 - begin1;
  input[0].valid = true;
  for (uint64_t i = 0; i < input[0].size; i++) {
    input[0].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  input[1].address = begin2;
  input[1].size = begin3 - begin2;
  input[1].valid = true;
  for (uint64_t i = 0; i < input[1].size; i++) {
    input[1].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  input[2].address = begin3;
  input[2].size = end - begin3;
  input[2].valid = true;
  for (uint64_t i = 0; i < input[2].size; i++) {
    input[2].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  MemoryDump dump(std::move(input));

  // Reading from the first block.
  std::array<uint8_t, 8> buf;
  constexpr std::array expected = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07};
  ASSERT_TRUE(dump.ReadBytes(begin1, buf.size(), buf.data()).ok());

  for (size_t i = 0; i < buf.size(); i++) {
    EXPECT_EQ(buf[i], expected[i]);
  }

  // Second block.
  buf = {};
  ASSERT_TRUE(dump.ReadBytes(begin2, buf.size(), buf.data()).ok());

  for (size_t i = 0; i < buf.size(); i++) {
    EXPECT_EQ(buf[i], expected[i]);
  }

  // Third block.
  buf = {};
  ASSERT_TRUE(dump.ReadBytes(begin3, buf.size(), buf.data()).ok());

  for (size_t i = 0; i < buf.size(); i++) {
    EXPECT_EQ(buf[i], expected[i]);
  }

  // Reading from anywhere in the block should also work.
  buf = {};
  ASSERT_TRUE(dump.ReadBytes(begin3 + 0x100, buf.size(), buf.data()).ok());

  for (size_t i = 0; i < buf.size(); i++) {
    EXPECT_EQ(buf[i], expected[i]);
  }
}

TEST(MemoryDump, ReadBytesErrors) {
  std::vector<debug_ipc::MemoryBlock> input;
  input.resize(3);

  uint64_t begin1 = 0x1000;
  uint64_t begin2 = 0x2000;
  uint64_t begin3 = 0x3000;
  uint64_t end = 0x4000;

  // Valid block filled with cycling bytes.
  input[0].address = begin1;
  input[0].size = begin2 - begin1;
  input[0].valid = true;
  for (uint64_t i = 0; i < input[1].size; i++) {
    input[0].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  input[1].address = begin2;
  input[1].size = begin3 - begin2;
  input[1].valid = true;
  for (uint64_t i = 0; i < input[1].size; i++) {
    input[1].data.push_back(static_cast<uint8_t>(i % 0x100));
  }

  // Invalid block.
  input[2].address = begin3;
  input[2].size = end - begin3;
  input[2].valid = false;

  MemoryDump dump(std::move(input));

  std::array<uint8_t, 8> buf;
  buf.fill(0);

  // Reading from the end address.
  EXPECT_TRUE(dump.ReadBytes(end, buf.size(), buf.data()).has_err());
  for (const auto& b : buf) {
    ASSERT_EQ(b, 0);
  }

  // Reading an address that isn't in the dump.
  EXPECT_TRUE(dump.ReadBytes(0x5000, buf.size(), buf.data()).has_err());
  for (const auto& b : buf) {
    ASSERT_EQ(b, 0);
  }

  // Request more bytes than any block can provide.
  EXPECT_TRUE(dump.ReadBytes(begin1, 0x10000, buf.data()).has_err());
  for (const auto& b : buf) {
    ASSERT_EQ(b, 0);
  }

  // Reading across the boundaries of different blocks will fail.
  EXPECT_TRUE(dump.ReadBytes(begin2 - 4, buf.size(), buf.data()).has_err());
  for (const auto& b : buf) {
    ASSERT_EQ(b, 0);
  }

  // Reading from invalid blocks will fail.
  EXPECT_TRUE(dump.ReadBytes(begin3, buf.size(), buf.data()).has_err());
  for (const auto& b : buf) {
    ASSERT_EQ(b, 0);
  }
}

}  // namespace zxdb
