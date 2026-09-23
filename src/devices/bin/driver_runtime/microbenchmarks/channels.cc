// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/channel.h>

#include <format>

#include <perftest/perftest.h>

#include "src/devices/bin/driver_runtime/microbenchmarks/assert.h"

namespace {

// Measure the times taken to enqueue and then dequeue a message from a
// Driver Runtime channel, on a single thread.  This does not involve any
// cross-thread wakeups.
bool ChannelWriteReadTest(perftest::RepeatState* state, uint32_t message_size,
                          uint32_t handle_count) {
  state->DeclareStep("write");
  state->DeclareStep("read");

  auto channel_pair = fdf::ChannelPair::Create(0);
  ASSERT_OK(channel_pair.status_value());

  constexpr uint32_t kTag = 'BNCH';
  fdf::Arena arena(kTag);

  void* data = arena.Allocate(message_size);
  auto handles_buf = static_cast<zx_handle_t*>(arena.Allocate(handle_count * sizeof(zx_handle_t)));

  cpp20::span<zx_handle_t> handles(handles_buf, handle_count);
  for (auto& handle : handles) {
    fdf_handle_t peer;
    ASSERT_OK(fdf_channel_create(0, &handle, &peer));
    // We only need one end of the channel to transfer.
    fdf_handle_close(peer);
  }

  while (state->KeepRunning()) {
    auto status = channel_pair->end0.Write(0, arena, data, message_size, std::move(handles));
    ASSERT_OK(status.status_value());
    state->NextStep();
    auto read_return = channel_pair->end1.Read(0);
    ASSERT_OK(read_return.status_value());
    data = read_return->data;
    handles = std::move(read_return->handles);
  }

  for (auto& handle : handles) {
    fdf_handle_close(handle);
  }
  return true;
}

// Same as |ChannelWriteReadTest|, but reading is done using the C API.
bool ChannelWriteReadCTest(perftest::RepeatState* state, uint32_t message_size,
                           uint32_t handle_count) {
  state->DeclareStep("write");
  state->DeclareStep("read");

  auto channel_pair = fdf::ChannelPair::Create(0);
  ASSERT_OK(channel_pair.status_value());

  constexpr uint32_t kTag = 'BNCH';
  fdf::Arena arena(kTag);

  void* data = arena.Allocate(message_size);
  auto handles_buf = static_cast<zx_handle_t*>(arena.Allocate(handle_count * sizeof(zx_handle_t)));

  cpp20::span<zx_handle_t> handles(handles_buf, handle_count);
  for (auto& handle : handles) {
    fdf_handle_t peer;
    ASSERT_OK(fdf_channel_create(0, &handle, &peer));
    // We only need one end of the channel to transfer.
    fdf_handle_close(peer);
  }

  while (state->KeepRunning()) {
    auto status = channel_pair->end0.Write(0, arena, data, message_size, std::move(handles));
    ASSERT_OK(status.status_value());
    state->NextStep();

    fdf_arena_t* read_arena;
    uint32_t num_bytes;
    zx_handle_t* read_handles;
    uint32_t num_handles;
    auto read_status = fdf_channel_read(channel_pair->end1.get(), 0, &read_arena, &data, &num_bytes,
                                        &read_handles, &num_handles);
    ASSERT_OK(read_status);
    fdf::Arena arena(read_arena);
    handles = cpp20::span{read_handles, num_handles};
  }

  for (auto& handle : handles) {
    fdf_handle_close(handle);
  }
  return true;
}

void RegisterTests() {
  static const unsigned kMessageSizesInBytes[] = {
      64,
      1 * 1024,
      32 * 1024,
      64 * 1024,
  };
  static const unsigned kHandleCounts[] = {
      0,
      1,
  };
  for (auto message_size : kMessageSizesInBytes) {
    for (auto handle_count : kHandleCounts) {
      auto write_read_name =
          std::format("Channel/WriteRead/{}bytes/{}handles", message_size, handle_count);
      perftest::RegisterTest(write_read_name.c_str(), ChannelWriteReadTest, message_size,
                             handle_count);
    }
  }
  for (auto message_size : kMessageSizesInBytes) {
    for (auto handle_count : kHandleCounts) {
      auto write_read_name =
          std::format("Channel/WriteReadC/{}bytes/{}handles", message_size, handle_count);
      perftest::RegisterTest(write_read_name.c_str(), ChannelWriteReadCTest, message_size,
                             handle_count);
    }
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
