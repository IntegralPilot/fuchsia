// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/cpu_profiler/fxt_writer.h"

#include <lib/fxt/serializer.h>

#include <gtest/gtest.h>

#include "zircon/types.h"

namespace profiler {

TEST(FxtWriterTest, WriteSingleRecord) {
  zx::socket socket1, socket2;
  ASSERT_EQ(zx::socket::create(0, &socket1, &socket2), ZX_OK);

  FxtWriter writer(std::move(socket1));
  const uint64_t header = 1234;
  zx::result<FxtRecordBuffer> buffer = writer.Reserve(header);
  ASSERT_TRUE(buffer.is_ok());

  buffer->WriteWord(1234);

  ASSERT_EQ(buffer->buffer_.size(), 2u);
  EXPECT_EQ(buffer->buffer_[1], 1234u);
}

TEST(FxtWriterTest, WriteBytes) {
  zx::socket socket1, socket2;
  ASSERT_EQ(zx::socket::create(0, &socket1, &socket2), ZX_OK);

  FxtWriter writer(std::move(socket1));
  zx::result<FxtRecordBuffer> buffer = writer.Reserve(0);
  ASSERT_TRUE(buffer.is_ok());

  const char* test_string = "Hello, world!";
  buffer->WriteBytes(test_string, strlen(test_string));

  ASSERT_EQ(buffer->buffer_.size(), 3u);
  EXPECT_EQ(buffer->buffer_[0], 0u);
  EXPECT_STREQ(reinterpret_cast<const char*>(&buffer->buffer_[1]), test_string);
}

TEST(FxtWriterTest, Commit) {
  zx::socket socket1, socket2;
  ASSERT_EQ(zx::socket::create(0, &socket1, &socket2), ZX_OK);

  FxtWriter writer(std::move(socket1));
  zx::result<FxtRecordBuffer> buffer = writer.Reserve(42);
  ASSERT_TRUE(buffer.is_ok());

  buffer->WriteWord(1234);
  buffer->Commit();

  std::vector<uint64_t> read_buffer(2);
  size_t actual;
  ASSERT_EQ(socket2.read(0, read_buffer.data(), read_buffer.size() * sizeof(uint64_t), &actual),
            ZX_OK);
  ASSERT_EQ(actual, 2 * sizeof(uint64_t));
  EXPECT_EQ(read_buffer[0], 42u);
  EXPECT_EQ(read_buffer[1], 1234u);
}

TEST(FxtWriterTest, WriteToSocket) {
  zx::socket socket1, socket2;
  ASSERT_EQ(zx::socket::create(0, &socket1, &socket2), ZX_OK);
  FxtWriter writer(std::move(socket1));
  zx_status_t status = fxt::WriteThreadRecord(&writer, 0, fxt::Koid(123), fxt::Koid(456));
  ASSERT_EQ(status, ZX_OK);

  std::vector<uint64_t> read_buffer(3);
  size_t actual;
  ASSERT_EQ(socket2.read(0, read_buffer.data(), read_buffer.size() * sizeof(uint64_t), &actual),
            ZX_OK);
  ASSERT_EQ(actual, 3 * sizeof(uint64_t));
  EXPECT_EQ(read_buffer[1], 123u);
  EXPECT_EQ(read_buffer[2], 456u);
}

}  // namespace profiler
