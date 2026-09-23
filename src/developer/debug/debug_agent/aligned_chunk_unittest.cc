// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/aligned_chunk.h"

#include <algorithm>

#include <gtest/gtest.h>

namespace debug_agent {

namespace {

std::vector<uint8_t> MakeSequence(size_t length, uint8_t first_value) {
  std::vector<uint8_t> result;
  for (size_t i = 0; i < length; i++)
    result.push_back(first_value + i);
  return result;
}

bool ReadFn(uint64_t addr, uint64_t* value) {
  // Expect all addresses to be word aligned.
  EXPECT_EQ(0u, addr % 8);
  const uint64_t* ptr = reinterpret_cast<const uint64_t*>(static_cast<intptr_t>(addr));
  *value = *ptr;
  return true;
}

bool WriteFn(uint64_t addr, uint64_t value) {
  // Expect all addresses to be word aligned.
  EXPECT_EQ(0u, addr % 8);
  uint64_t* ptr = reinterpret_cast<uint64_t*>(static_cast<intptr_t>(addr));
  *ptr = value;
  return true;
}

// Wraps "WriteAligned64Chunks".
bool CallWriteAligned(const void* src, size_t src_size, void* dest) {
  return WriteAligned64Chunks(src, src_size, &ReadFn, &WriteFn, reinterpret_cast<intptr_t>(dest));
}

}  // namespace

// Writing 0 bytes should not change anything.
TEST(AlignedChunk, EmptyWrite) {
  auto original = MakeSequence(10, 1);
  auto buf = original;
  EXPECT_TRUE(CallWriteAligned("", 0, buf.data()));

  EXPECT_EQ(original, buf);
}

TEST(AlignedChunk, Aligned) {
  auto buf = MakeSequence(32, 1);
  auto new_data = MakeSequence(16, 100);

  constexpr size_t kDestOffset = 8;

  auto expected = buf;
  std::copy(new_data.begin(), new_data.end(), expected.begin() + kDestOffset);

  EXPECT_TRUE(CallWriteAligned(new_data.data(), new_data.size(), &buf[kDestOffset]));
  EXPECT_EQ(expected, buf);
}

TEST(AlignedChunk, Unaligned) {
  auto new_data = MakeSequence(8, 100);

  // Try all the different offsets in the starting word.
  for (size_t i = 0; i < 8; i++) {
    auto buf = MakeSequence(32, 1);

    auto expected = buf;
    std::copy(new_data.begin(), new_data.end(), expected.begin() + i);

    EXPECT_TRUE(CallWriteAligned(new_data.data(), new_data.size(), &buf[i]));
    EXPECT_EQ(expected, buf);
  }
}

// Tests writing one byte to each position in a word.
TEST(AlignedChunk, OneByte) {
  const char new_data = 100;

  // Try all the different offsets in the starting word.
  for (size_t i = 0; i < 8; i++) {
    auto buf = MakeSequence(32, 1);

    auto expected = buf;
    expected[i] = new_data;

    EXPECT_TRUE(CallWriteAligned(&new_data, 1, &buf[i]));
    EXPECT_EQ(expected, buf);
  }
}

// Tests writing a sequence of three bytes to every offset within a word.
TEST(AlignedChunk, ThreeBytes) {
  auto new_data = MakeSequence(3, 100);

  // Try all the different offsets in the starting word.
  for (size_t i = 0; i < 8; i++) {
    auto buf = MakeSequence(32, 1);

    auto expected = buf;
    std::copy(new_data.begin(), new_data.end(), expected.begin() + i);

    EXPECT_TRUE(CallWriteAligned(new_data.data(), new_data.size(), &buf[i]));
    EXPECT_EQ(expected, buf);
  }
}

// When the read/write fails the function call should report that.
TEST(AlignedChunk, ReportsFailure) {
  const char new_data = 100;
  auto buf = MakeSequence(32, 1);

  // Read reports failure.
  EXPECT_FALSE(WriteAligned64Chunks(
      &new_data, 1, [](uint64_t addr, uint64_t*) { return false; }, &WriteFn,
      reinterpret_cast<intptr_t>(buf.data())));

  // Write reports failure.
  EXPECT_FALSE(WriteAligned64Chunks(
      &new_data, 1, &ReadFn, [](uint64_t, uint64_t) { return false; },
      reinterpret_cast<intptr_t>(buf.data())));
}

}  // namespace debug_agent
