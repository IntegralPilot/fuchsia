// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
#include <lib/zx/socket.h>

#include <vector>

#include <gtest/gtest.h>

#include "src/performance/trace_manager/buffer_forwarder.h"
#include "src/performance/trace_manager/deferred_buffer_forwarder.h"

TEST(BufferForwarderTest, DeferredForwarder) {
  zx::socket ep0, ep1;
  zx::socket::create(0, &ep0, &ep1);
  tracing::DeferredBufferForwarder forwarder(std::move(ep1));
  forwarder.WriteMagicNumberRecord();

  zx_signals_t pending;
  // We shouldn't see any data on the socket yet
  zx_status_t res = ep0.wait_one(ZX_SOCKET_READABLE, zx::time::infinite_past(), &pending);
  ASSERT_EQ(res, ZX_ERR_TIMED_OUT);

  forwarder.Flush();

  // Now we should see data
  res = ep0.wait_one(ZX_SOCKET_READABLE, zx::time::infinite_past(), &pending);
  ASSERT_EQ(res, ZX_OK);
  ASSERT_TRUE(pending & ZX_SOCKET_READABLE);

  uint64_t buffer[8];
  size_t actual;
  ep0.read(0, buffer, 64, &actual);

  // We should only read the 8 bytes of the record we wrote.
  ASSERT_EQ(actual, size_t{8});

  // And the bytes should be the FXT magic bytes.
  ASSERT_EQ(buffer[0], uint64_t{0x0016547846040010});

  // The socket should now be empty.
  res = ep0.wait_one(ZX_SOCKET_READABLE, zx::time::infinite_past(), &pending);
  ASSERT_EQ(res, ZX_ERR_TIMED_OUT);
}

TEST(BufferForwarderTest, SocketBackpressureThreshold) {
  zx::socket ep0, ep1;
  ASSERT_EQ(zx::socket::create(0, &ep0, &ep1), ZX_OK);

  zx::socket ep1_dup;
  ASSERT_EQ(ep1.duplicate(ZX_RIGHT_SAME_RIGHTS, &ep1_dup), ZX_OK);

  tracing::BufferForwarder forwarder(std::move(ep1));
  ASSERT_FALSE(forwarder.IsSocketBackpressureExceeded());

  zx_info_socket_t info;
  ASSERT_EQ(ep0.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr), ZX_OK);
  const size_t tx_buf_max = info.tx_buf_max;
  ASSERT_GT(tx_buf_max, 0u);

  const size_t threshold = (tx_buf_max * forwarder.backpressure_percentage()) / 100;
  ASSERT_GT(threshold, 0u);

  size_t bytes_to_write = threshold + 100;
  if (bytes_to_write > tx_buf_max) {
    bytes_to_write = tx_buf_max;
  }
  std::vector<uint8_t> mock_data(bytes_to_write, 0x55);
  size_t actual_written = 0;
  ASSERT_EQ(ep1_dup.write(0, mock_data.data(), mock_data.size(), &actual_written), ZX_OK);
  ASSERT_EQ(actual_written, bytes_to_write);

  ASSERT_TRUE(forwarder.IsSocketBackpressureExceeded());

  std::vector<uint8_t> read_buffer(bytes_to_write);
  size_t actual_read = 0;
  ASSERT_EQ(ep0.read(0, read_buffer.data(), read_buffer.size(), &actual_read), ZX_OK);
  ASSERT_EQ(actual_read, bytes_to_write);

  ASSERT_FALSE(forwarder.IsSocketBackpressureExceeded());
}
