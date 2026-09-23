// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/runtime/testing/cpp/internal/test_dispatcher_builder.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fdf/cpp/channel_read.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/testing.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/event.h>
#include <lib/zx/interrupt.h>

#include <format>
#include <map>

#include <perftest/perftest.h>

#include "src/devices/bin/driver_runtime/microbenchmarks/assert.h"

namespace {

/// Common interface between the runtime dispatcher and async loop.
class AsyncDispatcher {
 public:
  enum class Type {
    kRuntime,
    kAsyncLoop,
  };

  // If |use_threads_| is false, the dispatcher will be run on the current thread
  // until idle,
  virtual zx_status_t RunUntilIdleIfNoThreads() = 0;

  virtual ~AsyncDispatcher() {}

  virtual async_dispatcher_t* async_dispatcher() = 0;

 protected:
  explicit AsyncDispatcher(bool use_threads) : use_threads_(use_threads) {}

  bool use_threads_;
};

class RuntimeDispatcher : public AsyncDispatcher {
 public:
  RuntimeDispatcher(bool use_threads) : AsyncDispatcher(use_threads) {
    if (!use_threads) {
      // Make sure all dispatchers have completed destruction, otherwise |fdf_env_reset| will
      // complain.
      fdf_env_destroy_all_dispatchers();
      // Reset the runtime to 0 threads.
      fdf_env_reset();
    }

    zx::result<fdf::SynchronizedDispatcher> sync_dispatcher;
    if (use_threads) {
      sync_dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
          &fake_driver_, {}, "client",
          [&](fdf_dispatcher_t* dispatcher) { shutdown_completion_.Signal(); });
    } else {
      sync_dispatcher = fdf_internal::TestDispatcherBuilder::CreateUnmanagedSynchronizedDispatcher(
          &fake_driver_, {}, "client",
          [&](fdf_dispatcher_t* dispatcher) { shutdown_completion_.Signal(); });
    }
    ASSERT_OK(sync_dispatcher.status_value());
    dispatcher_ = *std::move(sync_dispatcher);
  }

  zx_status_t RunUntilIdleIfNoThreads() override {
    if (!use_threads_) {
      return fdf_testing_run_until_idle();
    }
    return ZX_OK;
  }

  ~RuntimeDispatcher() {
    dispatcher_.ShutdownAsync();
    ASSERT_OK(RunUntilIdleIfNoThreads());
    shutdown_completion_.Wait();

    // Make sure all dispatchers are destroyed, otherwise |fdf_env_start| will assert.
    dispatcher_.reset();

    if (!use_threads_) {
      // We stopped all the runtime threads, so make sure they are up again for the
      // rest of the benchmarks.
      ASSERT_OK(fdf_env_start(0));
    }
  }

  fdf_dispatcher_t* fdf_dispatcher() { return dispatcher_.get(); }
  async_dispatcher_t* async_dispatcher() override { return dispatcher_.async_dispatcher(); }

 private:
  uint32_t fake_driver_;
  libsync::Completion shutdown_completion_;

  fdf::Dispatcher dispatcher_;
};

class AsyncLoop : public AsyncDispatcher {
 public:
  explicit AsyncLoop(bool use_threads, bool enable_irqs) : AsyncDispatcher(use_threads) {
    async_loop_config_t config = kAsyncLoopConfigNoAttachToCurrentThread;
    config.irq_support = enable_irqs;
    loop_ = std::make_unique<async::Loop>(&config);
    if (use_threads_) {
      loop_->StartThread();
    }
  }

  zx_status_t RunUntilIdleIfNoThreads() override {
    if (!use_threads_) {
      return loop_->RunUntilIdle();
    }
    return ZX_OK;
  }

  ~AsyncLoop() {
    loop_->Quit();
    loop_->JoinThreads();
  }

  async_dispatcher_t* async_dispatcher() override { return loop_->dispatcher(); }

 private:
  std::unique_ptr<async::Loop> loop_;
};

std::unique_ptr<AsyncDispatcher> CreateAsyncDispatcher(AsyncDispatcher::Type type, bool use_threads,
                                                       bool enable_irqs) {
  switch (type) {
    case AsyncDispatcher::Type::kRuntime:
      return std::make_unique<RuntimeDispatcher>(use_threads);
    case AsyncDispatcher::Type::kAsyncLoop:
      return std::make_unique<AsyncLoop>(use_threads, enable_irqs);
    default:
      return nullptr;
  }
}

// Measure the time taken for a batch of tasks to be posted and completed.
bool DispatcherTaskTest(perftest::RepeatState* state, AsyncDispatcher::Type dispatcher_type,
                        bool use_threads, uint32_t batch_size) {
  auto dispatcher = CreateAsyncDispatcher(dispatcher_type, use_threads, false /* enable_irqs */);
  FX_CHECK(dispatcher);

  while (state->KeepRunning()) {
    libsync::Completion task_completion;
    for (uint32_t i = 0; i < batch_size; i++) {
      bool complete = (i == batch_size - 1);
      ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [complete, &task_completion] {
        if (complete) {
          task_completion.Signal();
        }
      }));
    }
    ASSERT_OK(dispatcher->RunUntilIdleIfNoThreads());
    task_completion.Wait();
  }

  return true;
}

// Measure the time taken for a wait callback to be completed.
bool DispatcherWaitTest(perftest::RepeatState* state, AsyncDispatcher::Type dispatcher_type,
                        bool use_threads) {
  auto dispatcher = CreateAsyncDispatcher(dispatcher_type, use_threads, false /* enable_irqs */);
  FX_CHECK(dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  libsync::Completion wait_completion;
  async::Wait wait(event.get(), ZX_USER_SIGNAL_0, 0,
                   async::Wait::Handler([&](async_dispatcher_t* dispatcher, async::Wait* wait,
                                            zx_status_t status, const zx_packet_signal_t* signal) {
                     ASSERT_OK(status);
                     ASSERT_OK(event.signal(ZX_USER_SIGNAL_0, 0));
                     wait_completion.Signal();
                   }));

  while (state->KeepRunning()) {
    ASSERT_OK(wait.Begin(dispatcher->async_dispatcher()));
    ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
    ASSERT_OK(dispatcher->RunUntilIdleIfNoThreads());
    wait_completion.Wait();
    wait_completion.Reset();
  }

  return true;
}

// Measure the time taken for a channel read to be completed.
bool DispatcherChannelReadTest(perftest::RepeatState* state, bool use_threads) {
  constexpr uint32_t kMsgSize = 4096;

  auto dispatcher = std::make_unique<RuntimeDispatcher>(use_threads);
  FX_CHECK(dispatcher);

  constexpr uint32_t kTag = 'BNCH';
  auto arena = fdf::Arena(kTag);
  auto msg = arena.Allocate(kMsgSize);

  auto channels = fdf::ChannelPair::Create(0);
  ASSERT_OK(channels.status_value());

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  libsync::Completion read_completion;
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      channels->end1.get(), 0 /* options */,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read,
          zx_status_t status) mutable {
        ASSERT_OK(status);
        FX_CHECK(channel_read->channel() == channels->end1.get());

        fdf_arena_t* read_arena;
        void* data;
        uint32_t num_bytes;
        zx_handle_t* read_handles;
        uint32_t num_handles;
        auto read_status = fdf_channel_read(channel_read->channel(), 0, &read_arena, &data,
                                            &num_bytes, &read_handles, &num_handles);
        ASSERT_OK(read_status);
        fdf::Arena arena(read_arena);
        read_completion.Signal();
      });

  while (state->KeepRunning()) {
    zx_status_t status = channel_read->Begin(dispatcher->fdf_dispatcher());
    ASSERT_OK(status);

    ASSERT_OK(
        channels->end0.Write(0, arena, msg, kMsgSize, cpp20::span<zx_handle_t>()).status_value());
    ASSERT_OK(dispatcher->RunUntilIdleIfNoThreads());
    read_completion.Wait();
    read_completion.Reset();
  }

  return true;
}

// Measure the time taken for a irq callback to be completed.
bool DispatcherIrqTest(perftest::RepeatState* state, AsyncDispatcher::Type dispatcher_type,
                       bool use_threads) {
  auto dispatcher = CreateAsyncDispatcher(dispatcher_type, use_threads, true /* enable_irqs */);
  FX_CHECK(dispatcher);

  zx::interrupt irq_object;
  ASSERT_OK(zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  libsync::Completion irq_completion;
  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) {
                   ASSERT_OK(status);
                   irq_object.ack();
                   irq_completion.Signal();
                 });

  ASSERT_OK(irq.Begin(dispatcher->async_dispatcher()));

  while (state->KeepRunning()) {
    irq_object.trigger(0, zx::time_boot());
    ASSERT_OK(dispatcher->RunUntilIdleIfNoThreads());
    irq_completion.Wait();
    irq_completion.Reset();
  }

  // Unbind from the dispatcher thread.
  libsync::Completion unbind_complete;
  ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&] {
    ASSERT_OK(irq.Cancel());
    unbind_complete.Signal();
  }));
  ASSERT_OK(dispatcher->RunUntilIdleIfNoThreads());
  unbind_complete.Wait();

  return true;
}

void RegisterTests() {
  std::map<AsyncDispatcher::Type, std::string> kDispatcherTypes = {
      {AsyncDispatcher::Type::kRuntime, "Runtime"},
      {AsyncDispatcher::Type::kAsyncLoop, "AsyncLoop"},
  };

  std::map<bool, std::string> kUseThreads = {
      {true, "Threads"},
      {false, "NoThreads"},
  };

  static const unsigned kTaskBatchSize[] = {
      1,
      10,
      100,
  };

  for (auto const& [type, dispatcher_name] : kDispatcherTypes) {
    for (auto const& [use_threads, use_threads_desc] : kUseThreads) {
      for (auto batch_size : kTaskBatchSize) {
        auto task_name = std::format("Dispatcher/{}/Task/{}/{}tasks", dispatcher_name,
                                     use_threads_desc, batch_size);
        perftest::RegisterTest(task_name.c_str(), DispatcherTaskTest, type, use_threads,
                               batch_size);
      }

      auto wait_name = std::format("Dispatcher/{}/Wait/{}", dispatcher_name, use_threads_desc);
      perftest::RegisterTest(wait_name.c_str(), DispatcherWaitTest, type, use_threads);

      auto irq_name = std::format("Dispatcher/{}/Irq/{}", dispatcher_name, use_threads_desc);
      perftest::RegisterTest(irq_name.c_str(), DispatcherIrqTest, type, use_threads);
    }
  }
  perftest::RegisterTest("Dispatcher/Runtime/ChannelRead/Threads", DispatcherChannelReadTest, true);
  perftest::RegisterTest("Dispatcher/Runtime/ChannelRead/NoThreads", DispatcherChannelReadTest,
                         false);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
