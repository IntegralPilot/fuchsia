// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fdf/cpp/channel_read.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/env.h>
#include <lib/fit/function.h>
#include <lib/sync/completion.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>

#include <format>
#include <map>
#include <vector>

#include "src/devices/bin/driver_runtime/microbenchmarks/assert.h"
#include "src/devices/bin/driver_runtime/microbenchmarks/test_runner.h"

namespace {

// Test one way transfers using fdf channels. The client writes |msg_count| messages,
// and the benchmark waits until the server has received all of them.
class OneWayTest {
 public:
  OneWayTest(uint32_t dispatcher_options, uint32_t msg_count, uint32_t msg_size)
      : msg_count_(msg_count), msg_size_(msg_size) {
    auto channel_pair = fdf::ChannelPair::Create(0);
    ASSERT_OK(channel_pair.status_value());

    client_ = std::move(channel_pair->end0);
    server_ = std::move(channel_pair->end1);

    {
      auto dispatcher = CreateDispatcher(&client_fake_driver_, dispatcher_options, "client");
      ASSERT_OK(dispatcher.status_value());
      client_dispatcher_ = *std::move(dispatcher);
    }

    {
      auto dispatcher = CreateDispatcher(&server_fake_driver_, dispatcher_options, "server");
      ASSERT_OK(dispatcher.status_value());
      server_dispatcher_ = *std::move(dispatcher);
    }

    constexpr uint32_t kTag = 'BNCH';
    arena_ = fdf::Arena(kTag);

    // Create the messages to transfer.
    for (uint32_t i = 0; i < msg_count_; i++) {
      msgs_.push_back(arena_.Allocate(msg_size_));
    }
    // TODO(https://fxbug.dev/42070233) migrate off this once setting a default dispatcher
    // with managed threads is possible.
    fdf_env_register_driver_entry(reinterpret_cast<const void*>(client_fake_driver_));
  }

  void Run() {
    uint32_t num_msgs_read = 0;
    libsync::Completion completion;
    auto channel_read = std::make_unique<fdf::ChannelRead>(
        server_.get(), 0 /* options */,
        [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read,
            zx_status_t status) mutable {
          ASSERT_OK(status);
          FX_CHECK(channel_read->channel() == server_.get());

          fdf_arena_t* read_arena;
          void* data;
          uint32_t num_bytes;
          zx_handle_t* read_handles;
          uint32_t num_handles;
          auto read_status = fdf_channel_read(channel_read->channel(), 0, &read_arena, &data,
                                              &num_bytes, &read_handles, &num_handles);
          ASSERT_OK(read_status);
          fdf::Arena arena(read_arena);

          num_msgs_read++;
          // If we haven't read enough messages yet, register the read again.
          if (num_msgs_read < msg_count_) {
            ASSERT_OK(channel_read->Begin(dispatcher));
          } else {
            // Test run is complete.
            completion.Signal();
          }
        });

    zx_status_t status = channel_read->Begin(server_dispatcher_.get());
    ASSERT_OK(status);

    // Send the messages from client to server.
    async_dispatcher_t* async_dispatcher = client_dispatcher_.async_dispatcher();
    FX_CHECK(async_dispatcher != nullptr);

    for (const auto& msg : msgs_) {
      ASSERT_OK(
          client_.Write(0, arena_, msg, msg_size_, cpp20::span<zx_handle_t>()).status_value());
    }
    completion.Wait();
    FX_CHECK(num_msgs_read == msg_count_);
  }

  void TearDown() {
    client_dispatcher_.ShutdownAsync();
    server_dispatcher_.ShutdownAsync();
    client_dispatcher_shutdown_.Wait();
    server_dispatcher_shutdown_.Wait();

    fdf_env_register_driver_exit();
  }

  void ShutdownHandler(fdf_dispatcher_t* dispatcher) {
    FX_CHECK((dispatcher == client_dispatcher_.get()) || (dispatcher == server_dispatcher_.get()));
    if (dispatcher == client_dispatcher_.get()) {
      client_dispatcher_shutdown_.Signal();
    } else {
      server_dispatcher_shutdown_.Signal();
    }
  }

 private:
  zx::result<fdf::Dispatcher> CreateDispatcher(void* owner, uint32_t options,
                                               std::string_view name) {
    if ((options & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
        FDF_DISPATCHER_OPTION_SYNCHRONIZED) {
      return fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
          owner, fdf::SynchronizedDispatcher::Options{.value = options}, name,
          fit::bind_member(this, &OneWayTest::ShutdownHandler));
    } else {
      return fdf_env::DispatcherBuilder::CreateUnsynchronizedWithOwner(
          owner, fdf::UnsynchronizedDispatcher::Options{.value = options}, name,
          fit::bind_member(this, &OneWayTest::ShutdownHandler));
    }
  }

  uint32_t msg_count_;
  uint32_t msg_size_;

  // Arena-allocated messages to transfer.
  std::vector<void*> msgs_;

  fdf::Channel client_;
  fdf::Dispatcher client_dispatcher_;
  libsync::Completion client_dispatcher_shutdown_;

  fdf::Channel server_;
  fdf::Dispatcher server_dispatcher_;
  libsync::Completion server_dispatcher_shutdown_;

  fdf::Arena arena_{nullptr};

  uint32_t client_fake_driver_;
  uint32_t server_fake_driver_;
};

// Test one way transfers using Zircon channels. The client writes |msg_count| messages,
// and the benchmark waits until the server has received all of them.
class OneWayZirconTest {
 public:
  OneWayZirconTest(uint32_t msg_count, uint32_t msg_size)
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        msg_count_(msg_count),
        msg_size_(msg_size),
        msg_buffer_(msg_size),
        read_buffer_(msg_size) {
    zx_status_t status = zx::channel::create(0, &client_, &server_);
    ASSERT_OK(status);

    loop_.StartThread();
  }

  void Run() {
    uint32_t num_msgs_read = 0;
    libsync::Completion completion;

    wait_ = std::make_unique<async::Wait>(
        server_.get(), ZX_CHANNEL_READABLE, 0,
        async::Wait::Handler([&, this](async_dispatcher_t* dispatcher, async::Wait* wait,
                                       zx_status_t status, const zx_packet_signal_t* signal) {
          uint32_t actual_bytes, actual_handles;
          ASSERT_OK(server_.read(0, read_buffer_.data(), nullptr, msg_size_, 0, &actual_bytes,
                                 &actual_handles));
          FX_CHECK(actual_bytes == msg_size_);

          num_msgs_read++;
          // If we haven't read enough messages yet, register the read again.
          if (num_msgs_read < msg_count_) {
            wait_->Begin(loop_.dispatcher());
          } else {
            // Test run is complete.
            completion.Signal();
          }
        }));

    ASSERT_OK(wait_->Begin(loop_.dispatcher()));

    for (uint32_t i = 0; i < msg_count_; i++) {
      ASSERT_OK(client_.write(0, msg_buffer_.data(), msg_size_, nullptr, 0));
    }

    completion.Wait();
  }

  void TearDown() {
    loop_.Quit();
    loop_.JoinThreads();
  }

 private:
  async::Loop loop_;

  uint32_t msg_count_;
  uint32_t msg_size_;

  std::vector<char> msg_buffer_;
  std::vector<char> read_buffer_;

  zx::channel client_;
  zx::channel server_;

  std::unique_ptr<async::Wait> wait_;
};

void RegisterTests() {
  std::map<uint32_t, std::string> kDispatcherTypes = {
      {FDF_DISPATCHER_OPTION_SYNCHRONIZED, "Inline_Synchronized"},
      {FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, "AllowSyncCalls"},
      {FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, "Inline_Unsynchronized"},
  };

  static const unsigned kMessageSizesInBytes[] = {
      64,
      64 * 1024,
  };
  static const unsigned kNumMessages[] = {
      1,
      1024,
  };
  for (auto message_size : kMessageSizesInBytes) {
    for (auto num_messages : kNumMessages) {
      for (auto const& [option, name] : kDispatcherTypes) {
        auto benchmark_name = std::format("OneWay_{}_{}_{}bytes", name, num_messages, message_size);
        driver_runtime_benchmark::RegisterTest<OneWayTest>(benchmark_name.c_str(), option,
                                                           num_messages, message_size);
      }
      auto benchmark_name = std::format("OneWay_Zircon_{}_{}bytes", num_messages, message_size);
      driver_runtime_benchmark::RegisterTest<OneWayZirconTest>(benchmark_name.c_str(), num_messages,
                                                               message_size);
    }
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
