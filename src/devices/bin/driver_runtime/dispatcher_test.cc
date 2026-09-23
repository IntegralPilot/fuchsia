// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_runtime/dispatcher.h"

#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/sequence_id.h>
#include <lib/async/task.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/arena.h>
#include <lib/fdf/channel.h>
#include <lib/fdf/cpp/channel_read.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/cpp/wake_vector.h>
#include <lib/fdf/internal.h>
#include <lib/fdf/testing.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/event.h>
#include <lib/zx/interrupt.h>
#include <zircon/errors.h>

#include <thread>

#include <zxtest/zxtest.h>

#include "lib/async/cpp/task.h"
#include "lib/fdf/dispatcher.h"
#include "lib/zx/time.h"
#include "src/devices/bin/driver_runtime/dispatcher_coordinator.h"
#include "src/devices/bin/driver_runtime/runtime_test_case.h"
#include "src/devices/bin/driver_runtime/thread_context.h"

namespace driver_runtime {
extern DispatcherCoordinator& GetDispatcherCoordinator();
}

class DispatcherTest : public RuntimeTestCase {
 public:
  void SetUp() override;
  void TearDown() override;

  // Creates a dispatcher and returns it in |out_dispatcher|.
  // The dispatcher will automatically be destroyed in |TearDown|.
  void CreateDispatcher(uint32_t options, std::string_view name, std::string_view scheduler_role,
                        const void* owner, fdf_dispatcher_t** out_dispatcher);
  void CreateUnmanagedDispatcher(uint32_t options, std::string_view name, const void* owner,
                                 fdf_dispatcher_t** out_dispatcher);

  // Starts a new thread on the default thread pool.
  // For tests which want to test running with a specific number of threads.
  void StartAdditionalManagedThread() {
    ASSERT_OK(
        driver_runtime::GetDispatcherCoordinator().default_thread_pool()->loop()->StartThread());
  }

  // Registers an async read, which on callback will acquire |lock| and read from |read_channel|.
  // If |reply_channel| is not null, it will write an empty message.
  // If |completion| is not null, it will signal before returning from the callback.
  static void RegisterAsyncReadReply(fdf_handle_t read_channel, fdf_dispatcher_t* dispatcher,
                                     fbl::Mutex* lock,
                                     fdf_handle_t reply_channel = ZX_HANDLE_INVALID,
                                     sync_completion_t* completion = nullptr);

  // Registers an async read, which on callback will acquire |lock|, read from |read_channel| and
  // signal |completion|.
  static void RegisterAsyncReadSignal(fdf_handle_t read_channel, fdf_dispatcher_t* dispatcher,
                                      fbl::Mutex* lock, sync_completion_t* completion) {
    return RegisterAsyncReadReply(read_channel, dispatcher, lock, ZX_HANDLE_INVALID, completion);
  }

  // Registers an async read, which on callback will signal |entered_callback| and block
  // until |complete_blocking_read| is signaled.
  static void RegisterAsyncReadBlock(fdf_handle_t ch, fdf_dispatcher_t* dispatcher,
                                     libsync::Completion* entered_callback,
                                     libsync::Completion* complete_blocking_read);

  static void WaitUntilIdle(fdf_dispatcher_t* dispatcher) {
    dispatcher->GetDispatcher()->WaitUntilIdle();
  }

  fdf_testing::internal::DriverRuntimeEnv runtime_env;

  fdf_handle_t local_ch_;
  fdf_handle_t remote_ch_;

  fdf_handle_t local_ch2_;
  fdf_handle_t remote_ch2_;

  std::vector<fdf_dispatcher_t*> dispatchers_;
  std::vector<std::unique_ptr<DispatcherShutdownObserver>> observers_;
};

// Like `DispatcherTest` but sets up the environment to spawn threads dynamically.
class DynamicEnvDispatcherTest : public DispatcherTest {
 public:
  void SetUp() override;
};

void DispatcherTest::SetUp() {
  // Make sure each test starts with exactly one thread.
  driver_runtime::GetDispatcherCoordinator().Reset();
  ASSERT_EQ(ZX_OK, driver_runtime::GetDispatcherCoordinator().Start(0));

  ASSERT_EQ(ZX_OK, fdf_channel_create(0, &local_ch_, &remote_ch_));
  ASSERT_EQ(ZX_OK, fdf_channel_create(0, &local_ch2_, &remote_ch2_));
}

void DynamicEnvDispatcherTest::SetUp() {
  // Make sure each test starts with exactly one thread.
  driver_runtime::GetDispatcherCoordinator().Reset();
  ASSERT_EQ(ZX_OK,
            driver_runtime::GetDispatcherCoordinator().Start(FDF_ENV_DYNAMIC_THREAD_SPAWNING));

  ASSERT_EQ(ZX_OK, fdf_channel_create(0, &local_ch_, &remote_ch_));
  ASSERT_EQ(ZX_OK, fdf_channel_create(0, &local_ch2_, &remote_ch2_));
}

void DispatcherTest::TearDown() {
  if (local_ch_) {
    fdf_handle_close(local_ch_);
  }
  if (remote_ch_) {
    fdf_handle_close(remote_ch_);
  }
  if (local_ch2_) {
    fdf_handle_close(local_ch2_);
  }
  if (remote_ch2_) {
    fdf_handle_close(remote_ch2_);
  }

  for (auto* dispatcher : dispatchers_) {
    fdf_dispatcher_shutdown_async(dispatcher);
  }
  fdf_testing_run_until_idle();
  for (auto& observer : observers_) {
    ASSERT_OK(observer->WaitUntilShutdown());
  }
  for (auto* dispatcher : dispatchers_) {
    fdf_dispatcher_destroy(dispatcher);
  }
}

void DispatcherTest::CreateDispatcher(uint32_t options, std::string_view name,
                                      std::string_view scheduler_role, const void* owner,
                                      fdf_dispatcher_t** out_dispatcher) {
  auto observer = std::make_unique<DispatcherShutdownObserver>();
  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(owner, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(options, name, scheduler_role,
                                                        observer->fdf_observer(), &dispatcher));
  }
  *out_dispatcher = dispatcher->to_fdf_dispatcher();
  dispatchers_.push_back(*out_dispatcher);
  observers_.push_back(std::move(observer));
}

void DispatcherTest::CreateUnmanagedDispatcher(uint32_t options, std::string_view name,
                                               const void* owner,
                                               fdf_dispatcher_t** out_dispatcher) {
  auto observer = std::make_unique<DispatcherShutdownObserver>();
  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(owner, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::CreateUnmanagedDispatcher(
                         options, name, observer->fdf_observer(), &dispatcher));
  }
  *out_dispatcher = dispatcher->to_fdf_dispatcher();
  dispatchers_.push_back(*out_dispatcher);
  observers_.push_back(std::move(observer));
}

// static
void DispatcherTest::RegisterAsyncReadReply(fdf_handle_t read_channel, fdf_dispatcher_t* dispatcher,
                                            fbl::Mutex* lock, fdf_handle_t reply_channel,
                                            sync_completion_t* completion) {
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      read_channel, 0 /* options */,
      [=](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);

        {
          fbl::AutoLock auto_lock(lock);

          ASSERT_NO_FATAL_FAILURE(AssertRead(channel_read->channel(), nullptr, 0, nullptr, 0));
          if (reply_channel != ZX_HANDLE_INVALID) {
            ASSERT_EQ(ZX_OK, fdf_channel_write(reply_channel, 0, nullptr, nullptr, 0, nullptr, 0));
          }
        }
        if (completion) {
          sync_completion_signal(completion);
        }
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(dispatcher));
  channel_read.release();  // Deleted on callback.
}

// static
void DispatcherTest::RegisterAsyncReadBlock(fdf_handle_t ch, fdf_dispatcher_t* dispatcher,
                                            libsync::Completion* entered_callback,
                                            libsync::Completion* complete_blocking_read) {
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      ch, 0 /* options */,
      [=](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);
        entered_callback->Signal();
        ASSERT_OK(complete_blocking_read->Wait(zx::time::infinite()));
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(dispatcher));
  channel_read.release();  // Will be deleted on callback.
}

//
// Synchronous dispatcher tests
//

// Tests that a synchronous dispatcher will call directly into the next driver
// if it is not reentrant.
// This creates 2 drivers and writes a message between them.
TEST_F(DispatcherTest, SyncDispatcherDirectCall) {
  const void* local_driver = CreateFakeDriver();
  const void* remote_driver = CreateFakeDriver();

  // We should bypass the async loop, so use an unmanaged dispatcher.
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, local_driver, &dispatcher));

  sync_completion_t read_completion;
  ASSERT_NO_FATAL_FAILURE(SignalOnChannelReadable(local_ch_, dispatcher, &read_completion));

  {
    thread_context::PushDriver(remote_driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    // As |local_driver| is not in the thread's call stack,
    // this should call directly into local driver's channel_read callback,
    // so do not call |fdf_testing_run_until_idle| here.
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
    ASSERT_OK(sync_completion_wait(&read_completion, ZX_TIME_INFINITE));
  }
}

// Tests that a synchronous dispatcher will queue a request on the async loop if it is reentrant.
// This writes and reads a message from the same driver.
TEST_F(DispatcherTest, SyncDispatcherCallOnLoop) {
  const void* driver = CreateFakeDriver();

  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, driver, &dispatcher));

  sync_completion_t read_completion;
  ASSERT_NO_FATAL_FAILURE(SignalOnChannelReadable(local_ch_, dispatcher, &read_completion));

  {
    // Add the same driver to the thread's call stack.
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    // This should queue the callback to run on an async loop thread.
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
    // Check that the callback hasn't been called yet, as we shutdown the async loop.
    ASSERT_FALSE(sync_completion_signaled(&read_completion));
    ASSERT_EQ(1, dispatcher->GetDispatcher()->callback_queue_size_slow());
  }

  ASSERT_OK(fdf_testing_run_until_idle());
  ASSERT_OK(sync_completion_wait(&read_completion, ZX_TIME_INFINITE));
}

// Tests that a synchronous dispatcher only allows one callback to be running at a time.
// We will register a callback that blocks and one that doesn't. We will then send
// 2 requests, and check that the second callback is not run until the first returns.
TEST_F(DispatcherTest, SyncDispatcherDisallowsParallelCallbacks) {
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

  // We shouldn't actually block on a dispatcher that doesn't have ALLOW_SYNC_CALLS set,
  // but this is just for synchronizing the test.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadBlock(local_ch_, dispatcher, &entered_callback, &complete_blocking_read));

  sync_completion_t read_completion;
  ASSERT_NO_FATAL_FAILURE(SignalOnChannelReadable(local_ch2_, dispatcher, &read_completion));

  {
    // This should make the callback run on the async loop, as it would be reentrant.
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  // Write another request. This should also be queued on the async loop.
  std::thread t1 = std::thread([&] {
    // Make the call not reentrant.
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch2_, 0, nullptr, nullptr, 0, nullptr, 0));
  });

  // The dispatcher should not call the callback while there is an existing callback running,
  // so we should be able to join with the thread immediately.
  t1.join();
  ASSERT_FALSE(sync_completion_signaled(&read_completion));

  // Complete the first callback.
  complete_blocking_read.Signal();

  // The second callback should complete now.
  ASSERT_OK(sync_completion_wait(&read_completion, ZX_TIME_INFINITE));
}

// Tests that a synchronous dispatcher does not schedule parallel callbacks on the async loop.
TEST_F(DispatcherTest, SyncDispatcherDisallowsParallelCallbacksReentrant) {
  constexpr uint32_t kNumThreads = 2;
  constexpr uint32_t kNumClients = 12;

  fdf_env_reset();

  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

  struct ReadClient {
    fdf_handle_t channel;
    libsync::Completion entered_callback;
    libsync::Completion complete_blocking_read;
  };

  std::vector<ReadClient> local(kNumClients);
  std::vector<fdf_handle_t> remote(kNumClients);

  for (uint32_t i = 0; i < kNumClients; i++) {
    ASSERT_OK(fdf_channel_create(0, &local[i].channel, &remote[i]));
    ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadBlock(local[i].channel, dispatcher,
                                                   &local[i].entered_callback,
                                                   &local[i].complete_blocking_read));
  }

  for (uint32_t i = 0; i < kNumClients; i++) {
    // Call is considered reentrant and will be queued on the async loop.
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote[i], 0, nullptr, nullptr, 0, nullptr, 0));
  }

  for (uint32_t i = 0; i < kNumThreads; i++) {
    StartAdditionalManagedThread();
  }

  ASSERT_OK(local[0].entered_callback.Wait(zx::time::infinite()));
  local[0].complete_blocking_read.Signal();

  // Check that we aren't blocking the second thread by posting a task to another
  // dispatcher.
  fdf_dispatcher_t* dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher2));
  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher2);
  ASSERT_NOT_NULL(async_dispatcher);

  sync_completion_t task_completion;
  ASSERT_OK(async::PostTask(async_dispatcher,
                            [&task_completion] { sync_completion_signal(&task_completion); }));
  ASSERT_OK(sync_completion_wait(&task_completion, ZX_TIME_INFINITE));

  // Allow all the read callbacks to complete.
  for (uint32_t i = 1; i < kNumClients; i++) {
    local[i].complete_blocking_read.Signal();
  }

  for (uint32_t i = 0; i < kNumClients; i++) {
    ASSERT_OK(local[i].entered_callback.Wait(zx::time::infinite()));
  }

  WaitUntilIdle(dispatcher);
  WaitUntilIdle(dispatcher2);

  for (uint32_t i = 0; i < kNumClients; i++) {
    fdf_handle_close(local[i].channel);
    fdf_handle_close(remote[i]);
  }
}

//
// Unsynchronized dispatcher tests
//

// Tests that an unsynchronized dispatcher allows multiple callbacks to run at the same time.
// We will send requests from multiple threads and check that the expected number of callbacks
// is running.
TEST_F(DispatcherTest, UnsyncDispatcherAllowsParallelCallbacks) {
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(
      CreateDispatcher(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, __func__, "", driver, &dispatcher));

  constexpr uint32_t kNumClients = 10;

  std::vector<fdf_handle_t> local(kNumClients);
  std::vector<fdf_handle_t> remote(kNumClients);

  for (uint32_t i = 0; i < kNumClients; i++) {
    ASSERT_OK(fdf_channel_create(0, &local[i], &remote[i]));
  }

  fbl::Mutex callback_lock;
  uint32_t num_callbacks = 0;
  sync_completion_t completion;

  for (uint32_t i = 0; i < kNumClients; i++) {
    auto channel_read = std::make_unique<fdf::ChannelRead>(
        local[i], 0 /* options */,
        [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
          {
            fbl::AutoLock lock(&callback_lock);
            num_callbacks++;
            if (num_callbacks == kNumClients) {
              sync_completion_signal(&completion);
            }
          }
          // Wait for all threads to ensure we are correctly supporting parallel callbacks.
          ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
          delete channel_read;
        });
    ASSERT_OK(channel_read->Begin(dispatcher));
    channel_read.release();  // Deleted by the callback.
  }

  std::vector<std::thread> threads;
  for (uint32_t i = 0; i < kNumClients; i++) {
    std::thread client = std::thread(
        [&](fdf_handle_t channel) {
          {
            // Ensure the call is not reentrant.
            thread_context::PushDriver(CreateFakeDriver());
            auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
            ASSERT_EQ(ZX_OK, fdf_channel_write(channel, 0, nullptr, nullptr, 0, nullptr, 0));
          }
        },
        remote[i]);
    threads.push_back(std::move(client));
  }

  for (auto& t : threads) {
    t.join();
  }

  for (uint32_t i = 0; i < kNumClients; i++) {
    fdf_handle_close(local[i]);
    fdf_handle_close(remote[i]);
  }
}

// Tests that an unsynchronized dispatcher allows multiple callbacks to run at the same time
// on the async loop.
// TODO(467410367): Re-enable when the test is no longer flaking.
TEST_F(DispatcherTest, DISABLED_UnsyncDispatcherAllowsParallelCallbacksReentrant) {
  fdf_env_reset();

  constexpr uint32_t kNumThreads = 3;
  constexpr uint32_t kNumClients = 22;

  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(
      CreateDispatcher(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, __func__, "", driver, &dispatcher));

  std::vector<fdf_handle_t> local(kNumClients);
  std::vector<fdf_handle_t> remote(kNumClients);

  for (uint32_t i = 0; i < kNumClients; i++) {
    ASSERT_OK(fdf_channel_create(0, &local[i], &remote[i]));
  }

  fbl::Mutex callback_lock;
  uint32_t num_callbacks = 0;
  sync_completion_t all_threads_running;

  for (uint32_t i = 0; i < kNumClients; i++) {
    auto channel_read = std::make_unique<fdf::ChannelRead>(
        local[i], 0 /* options */,
        [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
          {
            fbl::AutoLock lock(&callback_lock);
            num_callbacks++;
            if (num_callbacks == kNumThreads) {
              sync_completion_signal(&all_threads_running);
            }
          }
          // Wait for all threads to ensure we are correctly supporting parallel callbacks.
          ASSERT_OK(sync_completion_wait(&all_threads_running, ZX_TIME_INFINITE));
          delete channel_read;
        });
    ASSERT_OK(channel_read->Begin(dispatcher));
    channel_read.release();  // Deleted by the callback.
  }

  for (uint32_t i = 0; i < kNumClients; i++) {
    // Call is considered reentrant and will be queued on the async loop.
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote[i], 0, nullptr, nullptr, 0, nullptr, 0));
  }

  for (uint32_t i = 0; i < kNumThreads; i++) {
    StartAdditionalManagedThread();
  }

  ASSERT_OK(sync_completion_wait(&all_threads_running, ZX_TIME_INFINITE));
  WaitUntilIdle(dispatcher);
  ASSERT_EQ(num_callbacks, kNumClients);

  for (uint32_t i = 0; i < kNumClients; i++) {
    fdf_handle_close(local[i]);
    fdf_handle_close(remote[i]);
  }
}

//
// Blocking dispatcher tests
//

// Tests that a blocking dispatcher will not directly call into the next driver.
TEST_F(DispatcherTest, AllowSyncCallsDoesNotDirectlyCall) {
  const void* blocking_driver = CreateFakeDriver();
  fdf_dispatcher_t* blocking_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           blocking_driver, &blocking_dispatcher));

  // Queue a blocking request.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadBlock(remote_ch_, blocking_dispatcher, &entered_callback,
                                                 &complete_blocking_read));

  {
    // Simulate a driver writing a message to the driver with the blocking dispatcher.
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    // This is a non reentrant call, but we still shouldn't call into the driver directly.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  // Signal and wait for the blocking read handler to return.
  complete_blocking_read.Signal();

  WaitUntilIdle(blocking_dispatcher);
}

// Tests that dispatchers that allow sync calls can do inlined (direct) calls between each
// other.
TEST_F(DispatcherTest, AllowSyncCallsDirectCalls) {
  const void* driver_a = CreateFakeDriver();
  const void* driver_b = CreateFakeDriver();
  const void* driver_c = CreateFakeDriver();

  fdf_dispatcher_t* dispatcher_a;
  fdf_dispatcher_t* dispatcher_b;
  fdf_dispatcher_t* dispatcher_c;
  // With direct calls we should bypass the async loop, so create unmanaged dispatchers.
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS,
                                                    __func__, driver_a, &dispatcher_a));
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS,
                                                    __func__, driver_b, &dispatcher_b));
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS,
                                                    __func__, driver_c, &dispatcher_c));

  // Set up channels for [driver A to driver B], and [driver B to driver C].
  auto channels_ab = fdf::ChannelPair::Create(0);
  ASSERT_OK(channels_ab.status_value());
  auto channels_bc = fdf::ChannelPair::Create(0);
  ASSERT_OK(channels_bc.status_value());

  // Message that driver C will send to driver B.
  const uint32_t expected_msg = 7;
  // The Channel::Call requires additional space allocated for the message's transaction id.
  const uint32_t expected_num_bytes = sizeof(fdf_txid_t) + sizeof(expected_msg);

  // On reading a message from driver A, driver B will call into driver C, then reply to driver A
  // with driver C's message.
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      channels_ab->end1.get(), 0,
      [driver_c_ch = channels_bc->end0.borrow(), expected_num_bytes](
          fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        fdf::UnownedChannel channel(channel_read->channel());
        auto read = channel->Read(0);
        ASSERT_OK(read.status_value());
        // Store the received Channel::Call txid.
        ASSERT_EQ(sizeof(fdf_txid_t), read->num_bytes);
        fdf_txid_t txid;
        memcpy(&txid, read->data, read->num_bytes);

        // Call into driver C.
        auto call = driver_c_ch->Call(0, zx::time::infinite(), read->arena, read->data,
                                      read->num_bytes, cpp20::span<zx_handle_t>());
        ASSERT_OK(call.status_value());
        ASSERT_EQ(expected_num_bytes, call->num_bytes);

        // Reply to driver A with the message from driver C. We can just reuse the
        // received buffer and overwrite the txid.
        memcpy(call->data, &txid, sizeof(txid));
        auto write =
            channel->Write(0, call->arena, call->data, call->num_bytes, cpp20::span<zx_handle_t>());
        ASSERT_OK(write.status_value());
      });
  ASSERT_OK(channel_read->Begin(dispatcher_b));

  // On reading a message from driver B, driver C will reply with the |expected_msg|.
  auto channel_read2 = std::make_unique<fdf::ChannelRead>(
      channels_bc->end1.get(), 0,
      [expected_msg](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read,
                     zx_status_t status) {
        fdf::UnownedChannel channel(channel_read->channel());
        auto read = channel->Read(0);
        ASSERT_OK(read.status_value());
        // Store the received Channel::Call txid.
        ASSERT_EQ(sizeof(fdf_txid_t), read->num_bytes);
        fdf_txid_t txid;
        memcpy(&txid, read->data, read->num_bytes);

        // Reply to driver B with the same txid, and expected test data.
        fdf::Arena arena = fdf::Arena('TEST');
        uint8_t* send_bytes = static_cast<uint8_t*>(arena.Allocate(sizeof(expected_num_bytes)));
        memcpy(send_bytes, &txid, sizeof(txid));
        memcpy(send_bytes + sizeof(fdf_txid_t), &expected_msg, sizeof(expected_msg));
        ASSERT_OK(
            channel->Write(0, arena, send_bytes, expected_num_bytes, cpp20::span<zx_handle_t>()));
      });
  ASSERT_OK(channel_read2->Begin(dispatcher_c));

  {
    // Simulate a driver writing a message to the driver with the blocking dispatcher.
    thread_context::PushDriver(driver_a, dispatcher_a->GetDispatcher());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    // Allocate space for the runtime to write the txid.
    fdf::Arena arena = fdf::Arena('TEST');
    uint8_t* send_bytes = static_cast<uint8_t*>(arena.Allocate(sizeof(fdf_txid_t)));
    auto call = channels_ab->end0.Call(0, zx::time::infinite(), arena, send_bytes,
                                       sizeof(fdf_txid_t), cpp20::span<zx_handle_t>());

    ASSERT_OK(call.status_value());
    ASSERT_EQ(expected_num_bytes, call->num_bytes);
    ASSERT_EQ(0, memcmp(&expected_msg, static_cast<uint8_t*>(call->data) + sizeof(fdf_txid_t),
                        sizeof(expected_msg)));
  }
}

// Tests that a blocking dispatcher will not directly call into the next driver, but after sealing
// the allow_sync option, it will.
TEST_F(DispatcherTest, AllowSyncCallsDoesNotDirectlyCallUntilSealed) {
  const void* blocking_driver = CreateFakeDriver();
  fdf_dispatcher_t* blocking_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           blocking_driver, &blocking_dispatcher));

  // Queue a blocking request.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadBlock(remote_ch_, blocking_dispatcher, &entered_callback,
                                                 &complete_blocking_read));

  {
    // Simulate a driver writing a message to the driver with the blocking dispatcher.
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    // This is a non reentrant call, but we still shouldn't call into the driver directly.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  // Signal and wait for the blocking read handler to return.
  complete_blocking_read.Signal();

  // RegisterAsyncReadBlock doesn't do a read on its callback so we have to read here so we have a
  // clear channel for the next write+read.
  ASSERT_NO_FATAL_FAILURE(AssertRead(remote_ch_, nullptr, 0, nullptr, 0));

  WaitUntilIdle(blocking_dispatcher);

  // Seal
  libsync::Completion seal_completion;
  async::PostTask(fdf_dispatcher_get_async_dispatcher(blocking_dispatcher),
                  [&seal_completion, blocking_dispatcher]() {
                    ASSERT_EQ(ZX_OK, fdf_dispatcher_seal(blocking_dispatcher,
                                                         FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS));
                    seal_completion.Signal();
                  });
  ASSERT_OK(seal_completion.Wait(zx::time::infinite()));

  WaitUntilIdle(blocking_dispatcher);

  // Queue a read that should be called into directly now that the dispatcher doesn't
  // allow sync calls.
  fbl::Mutex driver_lock;
  entered_callback.Reset();
  ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadSignal(remote_ch_, blocking_dispatcher, &driver_lock,
                                                  entered_callback.get()));

  {
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    // This should call directly into the channel_read callback.
    ASSERT_FALSE(entered_callback.signaled());
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));

    // Validate the read did happen. Try the lock as well since the read should have completed.
    fbl::AutoLock lock(&driver_lock);
    ASSERT_TRUE(entered_callback.signaled());
  }
}

// Tests that a blocking dispatcher does not block the global async loop shared between
// all dispatchers in a process.
// We will register a blocking callback, and ensure we can receive other callbacks
// at the same time.
TEST_F(DispatcherTest, AllowSyncCallsDoesNotBlockGlobalLoop) {
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

  const void* blocking_driver = CreateFakeDriver();
  fdf_dispatcher_t* blocking_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           blocking_driver, &blocking_dispatcher));

  fdf_handle_t blocking_local_ch, blocking_remote_ch;
  ASSERT_EQ(ZX_OK, fdf_channel_create(0, &blocking_local_ch, &blocking_remote_ch));

  // Queue a blocking read.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadBlock(blocking_remote_ch, blocking_dispatcher,
                                                 &entered_callback, &complete_blocking_read));

  // Write a message for the blocking dispatcher.
  {
    thread_context::PushDriver(blocking_driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(blocking_local_ch, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  sync_completion_t read_completion;
  ASSERT_NO_FATAL_FAILURE(SignalOnChannelReadable(remote_ch_, dispatcher, &read_completion));

  {
    // Write a message which will be read on the non-blocking dispatcher.
    // Make the call reentrant so that the request is queued for the async loop.
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(sync_completion_wait(&read_completion, ZX_TIME_INFINITE));
  ASSERT_NO_FATAL_FAILURE(AssertRead(remote_ch_, nullptr, 0, nullptr, 0));

  // Signal and wait for the blocking read handler to return.
  complete_blocking_read.Signal();

  WaitUntilIdle(dispatcher);
  WaitUntilIdle(blocking_dispatcher);

  fdf_handle_close(blocking_local_ch);
  fdf_handle_close(blocking_remote_ch);
}

// Waits for the read to be ready, periodically running the stall scanner.
class ReadWaiter : public fdf_env_stall_scanner_t {
 public:
  // |read_completion| is the read to wait for to be ready.
  ReadWaiter(sync_completion_t* read_completion)
      : fdf_env_stall_scanner_t(fdf_env_stall_scanner_t{.handler = &ReadWaiter::Handler}),
        read_completion_(read_completion) {
    fdf_env_register_stall_scanner(this);
  }

  ~ReadWaiter() { fdf_env_register_stall_scanner(nullptr); }

  static void Handler(fdf_env_stall_scanner_t* scanner, zx_duration_mono_t duration) {
    static_cast<ReadWaiter*>(scanner)->Scan(duration);
  }

  void Scan(zx_duration_mono_t wait_time) {
    while (true) {
      zx_status_t read_status = sync_completion_wait(read_completion_, wait_time);
      if (read_status == ZX_OK) {
        return;
      }
      // Read timed out so we should check if threads are stalled.
      ASSERT_EQ(ZX_ERR_TIMED_OUT, read_status);
      wait_time = fdf_env_scan_threads_for_stalls2();
      if (wait_time == 0) {
        // Wait for next |Scan| callback.
        return;
      }
    }
  }

  void WaitForReadReady() { Scan(0); }

 private:
  sync_completion_t* read_completion_;
};

// Tests that a blocking dispatcher does not block the global async loop shared between
// all dispatchers in a process when running on an environment set up with dynamic thread spawning
// and running the stall checker periodically.
// We will register a blocking callback, and ensure we can receive other callbacks
// at the same time.
TEST_F(DynamicEnvDispatcherTest, AllowSyncCallsDoesNotBlockGlobalLoopWithDynamicSpawning) {
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

  const void* blocking_driver = CreateFakeDriver();
  fdf_dispatcher_t* blocking_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           blocking_driver, &blocking_dispatcher));

  fdf_handle_t blocking_local_ch, blocking_remote_ch;
  ASSERT_EQ(ZX_OK, fdf_channel_create(0, &blocking_local_ch, &blocking_remote_ch));

  // Queue a blocking read.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadBlock(blocking_remote_ch, blocking_dispatcher,
                                                 &entered_callback, &complete_blocking_read));

  // Write a message for the blocking dispatcher.
  {
    thread_context::PushDriver(blocking_driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(blocking_local_ch, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  sync_completion_t read_completion;
  ASSERT_NO_FATAL_FAILURE(SignalOnChannelReadable(remote_ch_, dispatcher, &read_completion));

  {
    // Write a message which will be read on the non-blocking dispatcher.
    // Make the call reentrant so that the request is queued for the async loop.
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ReadWaiter read_waiter(&read_completion);
  read_waiter.WaitForReadReady();

  ASSERT_NO_FATAL_FAILURE(AssertRead(remote_ch_, nullptr, 0, nullptr, 0));

  // Signal and wait for the blocking read handler to return.
  complete_blocking_read.Signal();

  WaitUntilIdle(dispatcher);
  WaitUntilIdle(blocking_dispatcher);

  fdf_handle_close(blocking_local_ch);
  fdf_handle_close(blocking_remote_ch);
}

//
// Additional re-entrancy tests
//

// Tests sending a request to another driver and receiving a reply across a single channel.
TEST_F(DispatcherTest, ReentrancySimpleSendAndReply) {
  // Create a dispatcher for each end of the channel.
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, "", "", driver, &dispatcher));

  const void* driver2 = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, "", "", driver2, &dispatcher2));

  // Lock that is acquired by the first driver whenever it writes or reads from |local_ch_|.
  // We shouldn't need to lock in a synchronous dispatcher, but this is just for testing
  // that the dispatcher handles reentrant calls. If the dispatcher attempts to call
  // reentrantly, this test will deadlock.
  fbl::Mutex driver_lock;
  fbl::Mutex driver2_lock;
  sync_completion_t completion;

  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadSignal(local_ch_, dispatcher, &driver_lock, &completion));
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadReply(remote_ch_, dispatcher2, &driver2_lock, remote_ch_));

  {
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    fbl::AutoLock lock(&driver_lock);
    // This should call directly into the next driver. When the driver writes its reply,
    // the dispatcher should detect that it is reentrant and queue it to be run on the
    // async loop. This will allow |fdf_channel_write| to return and |driver_lock| will
    // be released.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

// Tests sending a request to another driver, who sends a request back into the original driver
// on a different channel.
TEST_F(DispatcherTest, ReentrancyMultipleDriversAndDispatchers) {
  // Driver will own |local_ch_| and |local_ch2_|.
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

  // Driver2 will own |remote_ch_| and |remote_ch2_|.
  const void* driver2 = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver2, &dispatcher2));

  // Lock that is acquired by the driver whenever it writes or reads from its channels.
  // We shouldn't need to lock in a synchronous dispatcher, but this is just for testing
  // that the dispatcher handles reentrant calls. If the dispatcher attempts to call
  // reentrantly, this test will deadlock.
  fbl::Mutex driver_lock;
  fbl::Mutex driver2_lock;
  sync_completion_t completion;

  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadSignal(local_ch2_, dispatcher, &driver_lock, &completion));
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadReply(remote_ch_, dispatcher2, &driver2_lock, remote_ch2_));

  {
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    fbl::AutoLock lock(&driver_lock);
    // This should call directly into the next driver. When the driver writes its reply,
    // the dispatcher should detect that it is reentrant and queue it to be run on the
    // async loop. This will allow |fdf_channel_write| to return and |driver_lock| will
    // be released.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

// Tests a driver sending a request to another channel it owns.
TEST_F(DispatcherTest, ReentrancyOneDriverMultipleChannels) {
  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

  // Lock that is acquired by the driver whenever it writes or reads from its channels.
  // We shouldn't need to lock in a synchronous dispatcher, but this is just for testing
  // that the dispatcher handles reentrant calls. If the dispatcher attempts to call
  // reentrantly, this test will deadlock.
  fbl::Mutex driver_lock;
  sync_completion_t completion;

  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadSignal(local_ch2_, dispatcher, &driver_lock, &completion));
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadReply(remote_ch_, dispatcher, &driver_lock, remote_ch2_));

  {
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    fbl::AutoLock lock(&driver_lock);
    // Every call callback in this driver will be reentrant and should be run on the async loop.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

// Tests forwarding a request across many drivers, before calling back into the original driver.
TEST_F(DispatcherTest, ReentrancyManyDrivers) {
  constexpr uint32_t kNumDrivers = 30;

  // Each driver i uses ch_to_prev[i] and ch_to_next[i] to communicate with the driver before and
  // after it, except ch_to_prev[0] and ch_to_next[kNumDrivers-1].
  std::vector<fdf_handle_t> ch_to_prev(kNumDrivers);
  std::vector<fdf_handle_t> ch_to_next(kNumDrivers);

  // Lock that is acquired by the driver whenever it writes or reads from its channels.
  // We shouldn't need to lock in a synchronous dispatcher, but this is just for testing
  // that the dispatcher handles reentrant calls. If the dispatcher attempts to call
  // reentrantly, this test will deadlock.
  std::vector<fbl::Mutex> driver_locks(kNumDrivers);

  for (uint32_t i = 0; i < kNumDrivers; i++) {
    const void* driver = CreateFakeDriver();
    fdf_dispatcher_t* dispatcher;
    ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver, &dispatcher));

    // Get the next driver's channel which is connected to the current driver's channel.
    // The last driver will be connected to the first driver.
    fdf_handle_t* peer = (i == kNumDrivers - 1) ? &ch_to_prev[0] : &ch_to_prev[i + 1];
    ASSERT_OK(fdf_channel_create(0, &ch_to_next[i], peer));
  }

  // Signal once the first driver is called into.
  sync_completion_t completion;
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadSignal(ch_to_prev[0], dispatchers_[0], &driver_locks[0], &completion));

  // Each driver will wait for a callback, then write a message to the next driver.
  for (uint32_t i = 1; i < kNumDrivers; i++) {
    ASSERT_NO_FATAL_FAILURE(
        RegisterAsyncReadReply(ch_to_prev[i], dispatchers_[i], &driver_locks[i], ch_to_next[i]));
  }

  {
    thread_context::PushDriver(dispatchers_[0]->GetDispatcher()->owner());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    fbl::AutoLock lock(&driver_locks[0]);
    // Write from the first driver.
    // This should call directly into the next |kNumDrivers - 1| drivers.
    ASSERT_EQ(ZX_OK, fdf_channel_write(ch_to_next[0], 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
  for (uint32_t i = 0; i < kNumDrivers; i++) {
    WaitUntilIdle(dispatchers_[i]);
  }
  for (uint32_t i = 0; i < kNumDrivers; i++) {
    fdf_handle_close(ch_to_prev[i]);
    fdf_handle_close(ch_to_next[i]);
  }
}

// Tests writing a request from an unknown driver context.
TEST_F(DispatcherTest, EmptyCallStack) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  sync_completion_t read_completion;
  ASSERT_NO_FATAL_FAILURE(SignalOnChannelReadable(local_ch_, dispatcher, &read_completion));

  {
    // Call without any recorded call stack.
    // This should queue the callback to run on an async loop thread.
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
    ASSERT_EQ(1, dispatcher->GetDispatcher()->callback_queue_size_slow());
    ASSERT_FALSE(sync_completion_signaled(&read_completion));
  }

  ASSERT_OK(fdf_testing_run_until_idle());
  ASSERT_OK(sync_completion_wait(&read_completion, ZX_TIME_INFINITE));
}

//
// Shutdown() tests
//

// Tests shutting down a synchronized dispatcher that has a pending channel read
// that does not have a corresponding channel write.
TEST_F(DispatcherTest, SyncDispatcherShutdownBeforeWrite) {
  libsync::Completion read_complete;
  DispatcherShutdownObserver observer;

  const void* driver = CreateFakeDriver();
  constexpr std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(0, "", scheduler_role,
                                                        observer.fdf_observer(), &dispatcher));
  }

  fdf::Dispatcher fdf_dispatcher(dispatcher->to_fdf_dispatcher());

  // Registered, but not yet ready to run.
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_EQ(status, ZX_ERR_CANCELED);
        read_complete.Signal();
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(fdf_dispatcher.get()));
  channel_read.release();

  fdf_dispatcher.ShutdownAsync();

  ASSERT_OK(read_complete.Wait(zx::time::infinite()));
  ASSERT_OK(observer.WaitUntilShutdown());
}

// Tests shutting down a synchronized dispatcher that has a pending async wait
// that hasn't been signaled yet.
TEST_F(DispatcherTest, SyncDispatcherShutdownBeforeSignaled) {
  libsync::Completion wait_complete;
  DispatcherShutdownObserver observer;

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);

  const void* driver = CreateFakeDriver();
  constexpr std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(0, "", scheduler_role,
                                                        observer.fdf_observer(), &dispatcher));
  }
  fdf::Dispatcher fdf_dispatcher(dispatcher->to_fdf_dispatcher());

  // Registered, but not yet signaled.
  async_dispatcher_t* async_dispatcher = dispatcher->GetAsyncDispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  ASSERT_OK(wait.Begin(async_dispatcher, [&wait_complete, event = std::move(event)](
                                             async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ASSERT_STATUS(status, ZX_ERR_CANCELED);
    wait_complete.Signal();
  }));

  // Shutdown the dispatcher, which should schedule cancellation of the channel read.
  dispatcher->ShutdownAsync();

  ASSERT_OK(wait_complete.Wait(zx::time::infinite()));
  ASSERT_OK(observer.WaitUntilShutdown());
}

// Tests shutting down an unsynchronized dispatcher.
TEST_F(DispatcherTest, UnsyncDispatcherShutdown) {
  libsync::Completion complete_task;
  libsync::Completion read_complete;

  DispatcherShutdownObserver observer;

  const void* driver = CreateFakeDriver();
  const std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, "",
                                                        scheduler_role, observer.fdf_observer(),
                                                        &dispatcher));
  }
  fdf::Dispatcher fdf_dispatcher(dispatcher->to_fdf_dispatcher());
  libsync::Completion task_started;
  // Post a task that will block until we signal it.
  ASSERT_OK(async::PostTask(fdf_dispatcher.async_dispatcher(), [&] {
    task_started.Signal();
    ASSERT_OK(complete_task.Wait(zx::time::infinite()));
  }));
  // Ensure the task has been started.
  ASSERT_OK(task_started.Wait(zx::time::infinite()));

  // Register a channel read, which should not be queued until the
  // write happens.
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_EQ(status, ZX_ERR_CANCELED);
        read_complete.Signal();
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(fdf_dispatcher.get()));
  channel_read.release();

  {
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    // This should be considered reentrant and be queued on the async loop.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  fdf_dispatcher.ShutdownAsync();

  // The cancellation should not happen until the task completes.
  ASSERT_FALSE(read_complete.signaled());
  complete_task.Signal();
  ASSERT_OK(read_complete.Wait(zx::time::infinite()));

  ASSERT_OK(observer.WaitUntilShutdown());
}

// Tests shutting down an unsynchronized dispatcher that has a pending channel read
// that does not have a corresponding channel write.
TEST_F(DispatcherTest, UnsyncDispatcherShutdownBeforeWrite) {
  libsync::Completion read_complete;
  DispatcherShutdownObserver observer;

  const void* driver = CreateFakeDriver();
  constexpr std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, "",
                                                        scheduler_role, observer.fdf_observer(),
                                                        &dispatcher));
  }

  fdf::Dispatcher fdf_dispatcher(dispatcher->to_fdf_dispatcher());

  // Registered, but not yet ready to run.
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_EQ(status, ZX_ERR_CANCELED);
        read_complete.Signal();
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(fdf_dispatcher.get()));
  channel_read.release();

  fdf_dispatcher.ShutdownAsync();

  ASSERT_OK(read_complete.Wait(zx::time::infinite()));
  ASSERT_OK(observer.WaitUntilShutdown());
}

// Tests shutting down a unsynchronized dispatcher that has a pending async wait
// that hasn't been signaled yet.
TEST_F(DispatcherTest, UnsyncDispatcherShutdownBeforeSignaled) {
  libsync::Completion wait_complete;
  DispatcherShutdownObserver observer;

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);

  const void* driver = CreateFakeDriver();
  constexpr std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, "",
                                                        scheduler_role, observer.fdf_observer(),
                                                        &dispatcher));
  }
  fdf::Dispatcher fdf_dispatcher(dispatcher->to_fdf_dispatcher());

  // Registered, but not yet signaled.
  async_dispatcher_t* async_dispatcher = dispatcher->GetAsyncDispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  ASSERT_OK(wait.Begin(async_dispatcher, [&wait_complete, event = std::move(event)](
                                             async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ASSERT_STATUS(status, ZX_ERR_CANCELED);
    wait_complete.Signal();
  }));

  // Shutdown the dispatcher, which should schedule cancellation of the channel read.
  dispatcher->ShutdownAsync();

  ASSERT_OK(wait_complete.Wait(zx::time::infinite()));
  ASSERT_OK(observer.WaitUntilShutdown());
}

// Tests shutting down an unsynchronized dispatcher from a channel read callback running
// on the async loop.
TEST_F(DispatcherTest, ShutdownDispatcherInAsyncLoopCallback) {
  const void* driver = CreateFakeDriver();
  std::string_view scheduler_role = "";

  DispatcherShutdownObserver dispatcher_observer;

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(
                         FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, "", scheduler_role,
                         dispatcher_observer.fdf_observer(), &dispatcher));
  }

  libsync::Completion completion;
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0 /* options */,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);
        fdf_dispatcher_shutdown_async(dispatcher);
        completion.Signal();
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(dispatcher->to_fdf_dispatcher()));
  channel_read.release();  // Deleted on callback.

  {
    // Make the write reentrant so it is scheduled to run on the async loop.
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(completion.Wait(zx::time::infinite()));

  ASSERT_OK(dispatcher_observer.WaitUntilShutdown());
  dispatcher->Destroy();
}

// Tests that attempting to shut down a dispatcher twice from callbacks does not crash.
TEST_F(DispatcherTest, ShutdownDispatcherFromTwoCallbacks) {
  DispatcherShutdownObserver observer;
  const void* driver = CreateFakeDriver();

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    // We will not use managed threads, so that the channel reads don't get scheduled
    // until after we shut down the dispatcher.
    ASSERT_EQ(ZX_OK,
              driver_runtime::Dispatcher::CreateUnmanagedDispatcher(
                  FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, "", observer.fdf_observer(), &dispatcher));
  }

  libsync::Completion completion;
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0 /* options */,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);
        fdf_dispatcher_shutdown_async(dispatcher);
        completion.Signal();
      });
  ASSERT_OK(channel_read->Begin(dispatcher->to_fdf_dispatcher()));

  libsync::Completion completion2;
  auto channel_read2 = std::make_unique<fdf::ChannelRead>(
      remote_ch2_, 0 /* options */,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);
        fdf_dispatcher_shutdown_async(dispatcher);
        completion2.Signal();
      });
  ASSERT_OK(channel_read2->Begin(dispatcher->to_fdf_dispatcher()));

  {
    // Make the writes reentrant so they are scheduled to run on the async loop.
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch2_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  ASSERT_OK(fdf_testing_run_until_idle());
  ASSERT_OK(completion.Wait(zx::time::infinite()));
  ASSERT_OK(completion2.Wait(zx::time::infinite()));
  ASSERT_OK(observer.WaitUntilShutdown());
  dispatcher->Destroy();
}

// Tests that queueing a ChannelRead while the dispatcher is shutting down fails.
TEST_F(DispatcherTest, ShutdownDispatcherQueueChannelReadCallback) {
  libsync::Completion wait_task_started;
  libsync::Completion shutdown_started;
  libsync::Completion read_complete;
  DispatcherShutdownObserver observer;

  const void* driver = CreateFakeDriver();
  std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK,
              driver_runtime::Dispatcher::Create(
                  FDF_DISPATCHER_OPTION_SYNCHRONIZED | FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, "",
                  scheduler_role, observer.fdf_observer(), &dispatcher));
  }

  // Post a task that won't finish until we have started so that the read below can't
  // run its callback until after we've started the shutdown process.
  async::PostTask(dispatcher->GetAsyncDispatcher(), [&]() {
    wait_task_started.Signal();
    ASSERT_OK(shutdown_started.Wait(zx::time::infinite()));
  });
  ASSERT_OK(wait_task_started.Wait(zx::time::infinite()));

  fdf::Dispatcher fdf_dispatcher(dispatcher->to_fdf_dispatcher());

  auto channel_read = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_EQ(status, ZX_ERR_CANCELED);
        // We should not be able to queue the read again.
        ASSERT_EQ(channel_read->Begin(dispatcher), ZX_ERR_UNAVAILABLE);
        read_complete.Signal();
        delete channel_read;
      });
  ASSERT_OK(channel_read->Begin(fdf_dispatcher.get()));
  channel_read.release();  // Deleted on callback.

  {
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    // This should be considered reentrant and be queued on the async loop.
    ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }

  fdf_dispatcher.ShutdownAsync();
  shutdown_started.Signal();

  ASSERT_OK(read_complete.Wait(zx::time::infinite()));
  ASSERT_OK(observer.WaitUntilShutdown());
}

TEST_F(DispatcherTest, ShutdownCallbackIsNotReentrant) {
  fbl::Mutex driver_lock;

  libsync::Completion completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) {
    {
      fbl::AutoLock lock(&driver_lock);
    }
    completion.Signal();
  };

  thread_context::PushDriver(CreateFakeDriver());
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  {
    fbl::AutoLock lock(&driver_lock);
    dispatcher->ShutdownAsync();
  }

  ASSERT_OK(completion.Wait(zx::time::infinite()));
}

TEST_F(DispatcherTest, ChannelPeerWriteDuringShutdown) {
  constexpr uint32_t kNumChannelPairs = 1000;

  libsync::Completion shutdown;
  auto shutdown_handler = [&](fdf_dispatcher_t* shutdown_dispatcher) { shutdown.Signal(); };

  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  // Create a bunch of channels, and register one end with the dispatcher to wait for
  // available channel reads.
  fdf::Channel local[kNumChannelPairs];
  fdf::Channel remote[kNumChannelPairs];
  for (uint32_t i = 0; i < kNumChannelPairs; i++) {
    auto channels_status = fdf::ChannelPair::Create(0);
    ASSERT_OK(channels_status.status_value());
    local[i] = std::move(channels_status->end0);
    remote[i] = std::move(channels_status->end1);

    auto channel_read = std::make_unique<fdf::ChannelRead>(
        local[i].get(), 0 /* options */,
        [=](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
          ASSERT_EQ(ZX_ERR_CANCELED, status);
          delete channel_read;
        });
    ASSERT_OK(channel_read->Begin(dispatcher->get()));
    channel_read.release();  // Will be deleted on callback.
  }

  dispatcher->ShutdownAsync();

  for (uint32_t i = 0; i < kNumChannelPairs; i++) {
    // This will write the packet to the peer channel and attempt to call |QueueRegisteredCallback|
    // on the dispatcher.
    fdf::Arena arena(nullptr);
    ASSERT_EQ(ZX_OK,
              remote[i].Write(0, arena, nullptr, 0, cpp20::span<zx_handle_t>()).status_value());
  }
  shutdown.Wait();
}

//
// async_dispatcher_t
//

// Tests that we can use the fdf_dispatcher_t as an async_dispatcher_t.
TEST_F(DispatcherTest, AsyncDispatcher) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  sync_completion_t completion;
  ASSERT_OK(
      async::PostTask(async_dispatcher, [&completion] { sync_completion_signal(&completion); }));
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

TEST_F(DispatcherTest, DelayedTask) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  sync_completion_t completion;
  ASSERT_OK(async::PostTaskForTime(
      async_dispatcher, [&completion] { sync_completion_signal(&completion); },
      zx::deadline_after(zx::msec(10))));
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

TEST_F(DispatcherTest, TasksDoNotCallDirectly) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  thread_context::PushDriver(CreateFakeDriver());
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  libsync::Completion completion;
  ASSERT_OK(async::PostTask(async_dispatcher, [&completion] { completion.Signal(); }));
  ASSERT_FALSE(completion.signaled());

  ASSERT_OK(fdf_testing_run_until_idle());
  completion.Wait();
}

TEST_F(DispatcherTest, DowncastAsyncDispatcher) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  ASSERT_EQ(fdf_dispatcher_downcast_async_dispatcher(async_dispatcher), dispatcher);
}

TEST_F(DispatcherTest, CancelTask) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  async::TaskClosure task;
  task.set_handler([] { ASSERT_FALSE(true); });
  ASSERT_OK(task.Post(async_dispatcher));

  ASSERT_OK(task.Cancel());  // Task should not be running yet.
}

TEST_F(DispatcherTest, CancelDelayedTask) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  async::TaskClosure task;
  task.set_handler([] { ASSERT_FALSE(true); });
  ASSERT_OK(task.PostForTime(async_dispatcher, zx::deadline_after(zx::sec(100))));

  ASSERT_OK(task.Cancel());  // Task should not be running yet.
}

TEST_F(DispatcherTest, CancelTaskNotYetPosted) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  async::TaskClosure task;
  task.set_handler([] { ASSERT_FALSE(true); });

  ASSERT_EQ(task.Cancel(), ZX_ERR_NOT_FOUND);  // Task should not be running yet.
}

TEST_F(DispatcherTest, CancelTaskAlreadyRunning) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  async::TaskClosure task;
  libsync::Completion completion;
  task.set_handler([&] {
    ASSERT_EQ(task.Cancel(), ZX_ERR_NOT_FOUND);  // Task is already running.
    completion.Signal();
  });
  ASSERT_OK(task.Post(async_dispatcher));
  ASSERT_OK(fdf_testing_run_until_idle());
  ASSERT_OK(completion.Wait(zx::time::infinite()));
}

TEST_F(DispatcherTest, AsyncWaitOnce) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  sync_completion_t completion;
  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  ASSERT_OK(wait.Begin(async_dispatcher, [&completion, &async_dispatcher](
                                             async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ASSERT_EQ(async_dispatcher, dispatcher);
    ASSERT_OK(status);
    sync_completion_signal(&completion);
  }));
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

TEST_F(DispatcherTest, AsyncWaitEdgeOnce) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  // Set the signal on the event before waiting.
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));

  sync_completion_t completion;
  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0, ZX_WAIT_ASYNC_EDGE);

  ASSERT_OK(wait.Begin(async_dispatcher, [&completion, &async_dispatcher](
                                             async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ASSERT_EQ(async_dispatcher, dispatcher);
    ASSERT_OK(status);
    sync_completion_signal(&completion);
  }));

  async::PostTask(async_dispatcher, [&] {
    // The wait shouldn't have completed here due to ZX_WAIT_ASYNC_EDGE. Clear the signal and
    // continue.
    EXPECT_FALSE(sync_completion_signaled(&completion));
    ASSERT_OK(event.signal(ZX_USER_SIGNAL_0, 0));

    async::PostTask(async_dispatcher, [&] {
      // The wait still shouldn't have completed here. Now set the signal again, and wait for the
      // handler to run.
      EXPECT_FALSE(sync_completion_signaled(&completion));
      ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
    });
  });

  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

TEST_F(DispatcherTest, CancelWait) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  ASSERT_OK(wait.Begin(async_dispatcher,
                       [](async_dispatcher_t* dispatcher, async::WaitOnce* wait, zx_status_t status,
                          const zx_packet_signal_t* signal) { ZX_ASSERT(false); }));
  ASSERT_OK(wait.Cancel());
}

TEST_F(DispatcherTest, CancelWaitFromWithinCanceledWait) {
  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher =
      fdf::SynchronizedDispatcher::Create({}, "", [](fdf_dispatcher_t* dispatcher) {});
  ASSERT_FALSE(dispatcher.is_error());

  async_dispatcher_t* async_dispatcher = dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  async::WaitOnce wait2(event.get(), ZX_USER_SIGNAL_0);

  ASSERT_OK(wait.Begin(async_dispatcher, [&](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ASSERT_EQ(status, ZX_ERR_CANCELED);
    wait2.Cancel();
  }));

  // We will cancel this wait from wait's handler, so we never expect it to complete.
  ASSERT_OK(wait2.Begin(async_dispatcher, [](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ZX_ASSERT(false);
  }));

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));

  driver_shutdown_completion.Wait();
}

TEST_F(DispatcherTest, CancelWaitRaceCondition) {
  // Regression test for https://fxbug.dev/42061372, a tricky race condition when cancelling a wait.
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  // Start a second thread as this race condition depends on the dispatcher being multi-threaded.
  StartAdditionalManagedThread();

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Run the body a bunch of times to increase the chances of hitting the race condition.
  for (int i = 0; i < 100; i++) {
    libsync::Completion completion;
    async::PostTask(async_dispatcher, [&] {
      async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
      ASSERT_OK(
          wait.Begin(async_dispatcher, [](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                          zx_status_t status, const zx_packet_signal_t* signal) {
            // Since we are going to cancel the wait, the callback should not be invoked.
            ZX_ASSERT(false);
          }));

      // Signal the event, which queues up the wait callback to be invoked.
      event.signal(0, ZX_USER_SIGNAL_0);

      // Cancel should always succeed. This is because the dispatcher is synchronized and should
      // appear to the user as if it is single-threaded. Since the wait is cancelled in the same
      // block as the event is signaled, the code never yields to the dispatcher and it never has a
      // chance to receive the event signal and invoke the callback. However, in our multi-threaded
      // dispatcher, it *is* possible that another thread will receive the signal and queue up the
      // callback to be invoked, so we need to handle this case without failing.
      //
      // In practice, when this test fails it's usually because it hits a debug assert in the
      // underlying async implementation in sdk/lib/async/wait.cc, rather than failing
      // this assert.
      ASSERT_OK(wait.Cancel());
      completion.Signal();
    });

    // Make sure all the async tasks finish before exiting the test.
    completion.Wait();
  }
}

TEST_F(DispatcherTest, GetCurrentDispatcherInWait) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  sync_completion_t completion;
  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  ASSERT_OK(wait.Begin(
      async_dispatcher,
      [&completion, &dispatcher](async_dispatcher_t* async_dispatcher, async::WaitOnce* wait,
                                 zx_status_t status, const zx_packet_signal_t* signal) {
        ASSERT_EQ(fdf_dispatcher_get_current_dispatcher(), dispatcher);
        ASSERT_OK(status);
        sync_completion_signal(&completion);
      }));
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
}

TEST_F(DispatcherTest, WaitSynchronized) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  // Create a second dispatcher which allows sync calls to force multiple threads.
  fdf_dispatcher_t* unused_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           CreateFakeDriver(), &unused_dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event1, event2;
  ASSERT_OK(zx::event::create(0, &event1));
  ASSERT_OK(zx::event::create(0, &event2));

  fbl::Mutex lock1, lock2;
  sync_completion_t completion1, completion2;

  async::WaitOnce wait1(event1.get(), ZX_USER_SIGNAL_0);
  ASSERT_OK(wait1.Begin(
      async_dispatcher,
      [&completion1, &lock1, &lock2](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                     zx_status_t status, const zx_packet_signal_t* signal) {
        // Take note of the order the locks are acquired here.
        {
          fbl::AutoLock al1(&lock1);
          fbl::AutoLock al2(&lock2);
        }
        sync_completion_signal(&completion1);
      }));
  async::WaitOnce wait2(event1.get(), ZX_USER_SIGNAL_0);
  ASSERT_OK(wait2.Begin(
      async_dispatcher,
      [&completion2, &lock1, &lock2](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                     zx_status_t status, const zx_packet_signal_t* signal) {
        // Locks acquired here in opposite order. If these calls are ever made in parallel, then we
        // run into a deadlock. The test should hang and eventually timeout in that case.
        {
          fbl::AutoLock al2(&lock2);
          fbl::AutoLock al1(&lock1);
        }
        sync_completion_signal(&completion2);
      }));

  // While the order of these signals are serialized, the order in which the signals are observed by
  // the waits is not. As a result either of the above waits may trigger first.
  ASSERT_OK(event1.signal(0, ZX_USER_SIGNAL_0));
  ASSERT_OK(event2.signal(0, ZX_USER_SIGNAL_0));
  // The order of observing these completions does not matter.
  ASSERT_OK(sync_completion_wait(&completion2, ZX_TIME_INFINITE));
  ASSERT_OK(sync_completion_wait(&completion1, ZX_TIME_INFINITE));
}

// Tests an irq can be bound and multiple callbacks received.
TEST_F(DispatcherTest, Irq) {
  static constexpr uint32_t kNumCallbacks = 10;

  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher));

  async_dispatcher_t* dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  ASSERT_NOT_NULL(dispatcher);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  libsync::Completion irq_signal;
  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) {
                   irq_object.ack();
                   ASSERT_EQ(irq_arg, &irq);
                   ASSERT_EQ(ZX_OK, status);
                   irq_signal.Signal();
                 });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));
  ASSERT_EQ(ZX_ERR_ALREADY_EXISTS, irq.Begin(dispatcher));

  for (uint32_t i = 0; i < kNumCallbacks; i++) {
    irq_object.trigger(0, zx::time_boot());
    irq_signal.Wait();
    irq_signal.Reset();
  }

  // Must unbind irq from dispatcher thread.
  libsync::Completion unbind_complete;
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    ASSERT_OK(irq.Cancel());
    ASSERT_EQ(ZX_ERR_NOT_FOUND, irq.Cancel());
    unbind_complete.Signal();
  }));
  unbind_complete.Wait();
}

// Tests that the client will stop receiving callbacks after unbinding the irq.
TEST_F(DispatcherTest, UnbindIrq) {
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher));

  async_dispatcher_t* dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  ASSERT_NOT_NULL(dispatcher);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) { ASSERT_FALSE(true); });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));

  // Must unbind irq from dispatcher thread.
  libsync::Completion unbind_complete;
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    ASSERT_OK(irq.Cancel());
    unbind_complete.Signal();
  }));
  unbind_complete.Wait();

  // The irq has been unbound, so this should not call the handler.
  irq_object.trigger(0, zx::time_boot());
}

// Tests that we get cancellation callbacks for irqs that are still bound when shutting down.
TEST_F(DispatcherTest, IrqCancelOnShutdown) {
  libsync::Completion completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { completion.Signal(); };

  thread_context::PushDriver(CreateFakeDriver());
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto fdf_dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
  ASSERT_FALSE(fdf_dispatcher.is_error());

  async_dispatcher_t* dispatcher = fdf_dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(dispatcher);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  libsync::Completion irq_completion;
  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) {
                   ASSERT_EQ(ZX_ERR_CANCELED, status);
                   irq_completion.Signal();
                 });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));

  // This should unbind the irq and call the handler with ZX_ERR_CANCELED.
  fdf_dispatcher->ShutdownAsync();
  irq_completion.Wait();
  ASSERT_OK(completion.Wait(zx::time::infinite()));
}

// Tests that we get one cancellation callback per irq that is still bound when shutting down.
TEST_F(DispatcherTest, IrqCancelOnShutdownCallbackOnlyOnce) {
  libsync::Completion shutdown_completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto fdf_dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      CreateFakeDriver(), {}, "", shutdown_handler);
  ASSERT_FALSE(fdf_dispatcher.is_error());

  async_dispatcher_t* dispatcher = fdf_dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(dispatcher);

  // Create a second dispatcher which allows sync calls to force multiple threads.
  libsync::Completion shutdown_completion2;
  auto shutdown_handler2 = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion2.Signal(); };
  auto fdf_dispatcher2 = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      CreateFakeDriver(), fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "",
      shutdown_handler2);
  ASSERT_FALSE(fdf_dispatcher2.is_error());

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  libsync::Completion irq_completion;
  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) {
                   ASSERT_FALSE(irq_completion.signaled());  // Make sure it is only called once.
                   ASSERT_EQ(ZX_ERR_CANCELED, status);
                   irq_completion.Signal();
                 });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));

  // Block the sync dispatcher thread with a task.
  libsync::Completion entered_task;
  libsync::Completion complete_task;
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    entered_task.Signal();
    complete_task.Wait();
  }));
  entered_task.Wait();

  // Trigger the irq to queue a callback request.
  irq_object.trigger(0, zx::time_boot());

  // Make sure the callback request has already been queued by the second global dispatcher thread,
  // by queueing a task after the trigger and waiting for the task's completion.
  libsync::Completion task_complete;
  ASSERT_OK(async::PostTask(fdf_dispatcher2->async_dispatcher(), [&] { task_complete.Signal(); }));
  task_complete.Wait();

  // This should remove the in-flight irq, unbind the irq and call the handler with ZX_ERR_CANCELED.
  fdf_dispatcher->ShutdownAsync();

  // We can now unblock the first dispatcher.
  complete_task.Signal();

  shutdown_completion.Wait();
  irq_completion.Wait();

  fdf_dispatcher2->ShutdownAsync();
  shutdown_completion2.Wait();
}

// Tests that an irq can be unbound after a dispatcher begins shutting down.
TEST_F(DispatcherTest, UnbindIrqAfterDispatcherShutdown) {
  libsync::Completion completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { completion.Signal(); };

  thread_context::PushDriver(CreateFakeDriver());
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto fdf_dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
  ASSERT_FALSE(fdf_dispatcher.is_error());

  async_dispatcher_t* dispatcher = fdf_dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(dispatcher);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) { ASSERT_TRUE(false); });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));

  ASSERT_OK(async::PostTask(dispatcher, [&] {
    fdf_dispatcher->ShutdownAsync();
    ASSERT_OK(irq.Cancel());
  }));

  ASSERT_OK(completion.Wait(zx::time::infinite()));
}

// Tests that when using a SYNCHRONIZED dispatcher, irqs are not delivered in parallel.
TEST_F(DispatcherTest, IrqSynchronized) {
  // Create a dispatcher that we will bind 2 irqs to.
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher));

  async_dispatcher_t* dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  ASSERT_NOT_NULL(dispatcher);

  // Create a second dispatcher which allows sync calls to force multiple threads.
  fdf_dispatcher_t* fdf_dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           CreateFakeDriver(), &fdf_dispatcher2));

  zx::interrupt irq_object1;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object1));
  zx::interrupt irq_object2;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object2));

  // We will bind 2 irqs to one dispatcher, and trigger them both. The irq handlers will block
  // until a task posted to another dispatcher completes. If the irqs callbacks happen
  // in parallel, the task will not be able to run, and the test will hang.
  libsync::Completion task_completion;
  libsync::Completion irq_completion1, irq_completion2;

  async::Irq irq1(irq_object1.get(), 0,
                  [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                      const zx_packet_interrupt_t* interrupt) {
                    task_completion.Wait();
                    irq_object1.ack();
                    ASSERT_EQ(irq_arg, &irq1);
                    ASSERT_EQ(ZX_OK, status);
                    irq_completion1.Signal();
                  });
  ASSERT_EQ(ZX_OK, irq1.Begin(dispatcher));

  async::Irq irq2(irq_object2.get(), 0,
                  [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                      const zx_packet_interrupt_t* interrupt) {
                    task_completion.Wait();
                    irq_object2.ack();
                    ASSERT_EQ(irq_arg, &irq2);
                    ASSERT_EQ(ZX_OK, status);
                    irq_completion2.Signal();
                  });
  ASSERT_EQ(ZX_OK, irq2.Begin(dispatcher));

  // While the order of these triggers are serialized, the order in which the triggers are observed
  // by the async_irqs is not. As a result either of the above async_irqs may trigger first. If the
  // irqs are not synchronized, both irq handlers will run and block.
  irq_object1.trigger(0, zx::time_boot());
  irq_object2.trigger(0, zx::time_boot());

  // Unblock the irq handler.
  ASSERT_OK(async::PostTask(fdf_dispatcher_get_async_dispatcher(fdf_dispatcher2),
                            [&] { task_completion.Signal(); }));
  task_completion.Wait();

  // The order of observing these completions does not matter.
  irq_completion2.Wait();
  irq_completion1.Wait();

  // Must unbind irqs from dispatcher thread.
  libsync::Completion unbind_complete;
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    ASSERT_OK(irq1.Cancel());
    ASSERT_OK(irq2.Cancel());
    unbind_complete.Signal();
  }));
  unbind_complete.Wait();
}

TEST_F(DispatcherTest, UnbindIrqRemovesPacketFromPort) {
  libsync::Completion completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { completion.Signal(); };

  thread_context::PushDriver(CreateFakeDriver());
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto fdf_dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
  ASSERT_FALSE(fdf_dispatcher.is_error());

  async_dispatcher_t* dispatcher = fdf_dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(dispatcher);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) { ASSERT_TRUE(false); });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));

  libsync::Completion task_complete;
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    // The irq handler should not be called yet since the dispatcher thread is blocked.
    irq_object.trigger(0, zx::time_boot());
    // This should remove the pending irq packet from the port.
    ASSERT_OK(irq.Cancel());
    task_complete.Signal();
  }));
  task_complete.Wait();

  fdf_dispatcher->ShutdownAsync();
  ASSERT_OK(completion.Wait(zx::time::infinite()));
}

TEST_F(DispatcherTest, UnbindIrqRemovesQueuedIrqs) {
  // Create a dispatcher that we will bind 2 irqs to.
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher));

  async_dispatcher_t* dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  ASSERT_NOT_NULL(dispatcher);

  // Create a second dispatcher which allows sync calls to force multiple threads.
  fdf_dispatcher_t* fdf_dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                           CreateFakeDriver(), &fdf_dispatcher2));

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) { ASSERT_FALSE(true); });
  ASSERT_EQ(ZX_OK, irq.Begin(dispatcher));

  // Block the dispatcher thread.
  libsync::Completion task_started;
  libsync::Completion complete_task;
  libsync::Completion task_complete;
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    task_started.Signal();
    // We will cancel the irq once the test has confirmed that the irq |OnSignal| has happened.
    complete_task.Wait();
    ASSERT_OK(irq.Cancel());
    task_complete.Signal();
  }));
  task_started.Wait();

  irq_object.trigger(0, zx::time_boot());

  // Make sure the irq |OnSignal| has happened on the other |process_shared_dispatcher| thread.
  // Since there are only 2 threads, and 1 is blocked by the task, the other must have already
  // processed the irq.
  libsync::Completion task2_completion;
  ASSERT_OK(async::PostTask(fdf_dispatcher_get_async_dispatcher(fdf_dispatcher2),
                            [&] { task2_completion.Signal(); }));
  task2_completion.Wait();

  complete_task.Signal();
  task_complete.Wait();

  // The task unbound the irq, so any queued irq callback request should be cancelled.
  // If not, the irq handler will be called and assert.
}

// Tests the potential race condition that occurs when an irq is unbound
// but the port has just read the irq packet from the port.
TEST_F(DispatcherTest, UnbindIrqImmediatelyAfterTriggering) {
  static constexpr uint32_t kNumIrqs = 3000;
  static constexpr uint32_t kNumThreads = 10;

  // TODO(https://fxbug.dev/42053861): this can be replaced by |fdf_env::DriverShutdown| once it
  // works properly.
  libsync::Completion shutdown_completion;
  std::atomic_int num_destructed = 0;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) {
    // |fetch_add| returns the value before incrementing.
    if (num_destructed.fetch_add(1) == kNumThreads - 1) {
      shutdown_completion.Signal();
    }
  };

  auto driver = CreateFakeDriver();
  thread_context::PushDriver(driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto fdf_dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
  ASSERT_FALSE(fdf_dispatcher.is_error());

  async_dispatcher_t* dispatcher = fdf_dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(dispatcher);

  // Create a bunch of blocking dispatchers to force new threads.
  fdf::Dispatcher unused_dispatchers[kNumThreads - 1];
  {
    for (uint32_t i = 0; i < kNumThreads - 1; i++) {
      auto fdf_dispatcher = fdf::SynchronizedDispatcher::Create(
          fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "", destructed_handler);
      ASSERT_FALSE(fdf_dispatcher.is_error());
      unused_dispatchers[i] = *std::move(fdf_dispatcher);
    }
  }

  // Create and unbind a bunch of irqs.
  zx::interrupt irqs[kNumIrqs] = {};
  std::vector<std::unique_ptr<async::Irq>> irq_wrappers(kNumIrqs);
  for (uint32_t i = 0; i < kNumIrqs; i++) {
    // Must unbind irq from dispatcher thread.
    libsync::Completion unbind_complete;
    ASSERT_OK(async::PostTask(dispatcher, [&, i] {
      ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irqs[i]));

      irq_wrappers[i] = std::make_unique<async::Irq>(
          irqs[i].get(), 0,
          [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
              const zx_packet_interrupt_t* interrupt) { ASSERT_FALSE(true); });
      ASSERT_EQ(ZX_OK, irq_wrappers[i]->Begin(dispatcher));
      // This queues the irq packet on the port, which may be read by another thread.
      irqs[i].trigger(0, zx::time_boot());
      ASSERT_OK(irq_wrappers[i]->Cancel());
      unbind_complete.Signal();
    }));
    unbind_complete.Wait();
  }

  fdf_dispatcher->ShutdownAsync();
  for (uint32_t i = 0; i < kNumThreads - 1; i++) {
    unused_dispatchers[i].ShutdownAsync();
  }
  shutdown_completion.Wait();

  fdf_dispatcher->reset();
  for (uint32_t i = 0; i < kNumThreads - 1; i++) {
    unused_dispatchers[i].reset();
  }
}

// Tests that binding irqs to an unsynchronized dispatcher is not allowed.
TEST_F(DispatcherTest, IrqUnsynchronized) {
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, __func__, "",
                                           CreateFakeDriver(), &fdf_dispatcher));

  async_dispatcher_t* dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  ASSERT_NOT_NULL(dispatcher);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  async::Irq irq(irq_object.get(), 0,
                 [&](async_dispatcher_t* dispatcher_arg, async::Irq* irq_arg, zx_status_t status,
                     const zx_packet_interrupt_t* interrupt) { ASSERT_TRUE(false); });
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, irq.Begin(dispatcher));
}

void IrqNotCalledHandler(async_dispatcher_t* async, async_irq_t* irq, zx_status_t status,
                         const zx_packet_interrupt_t* packet) {
  ASSERT_TRUE(status == ZX_ERR_CANCELED);
}

// Tests that you cannot unbind an irq from a different dispatcher from which it was bound to.
TEST_F(DispatcherTest, UnbindIrqFromWrongDispatcher) {
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher));

  async_dispatcher_t* dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  ASSERT_NOT_NULL(dispatcher);

  fdf_dispatcher_t* fdf_dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher2));

  async_dispatcher_t* dispatcher2 = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher2);
  ASSERT_NOT_NULL(dispatcher2);

  zx::interrupt irq_object;
  ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &irq_object));

  // Use the C API, as the C++ async::Irq will clear the dispatcher on the first call to Cancel.
  async_irq_t irq = {{ASYNC_STATE_INIT}, &IrqNotCalledHandler, irq_object.get()};

  ASSERT_OK(async_bind_irq(dispatcher, &irq));

  libsync::Completion task_complete;
  ASSERT_OK(async::PostTask(dispatcher2, [&] {
    // Cancel the irq from a different dispatcher it was bound to.
    ASSERT_EQ(ZX_ERR_BAD_STATE, async_unbind_irq(dispatcher, &irq));
    task_complete.Signal();
  }));
  task_complete.Wait();

  task_complete.Reset();
  ASSERT_OK(async::PostTask(dispatcher, [&] {
    ASSERT_EQ(ZX_OK, async_unbind_irq(dispatcher, &irq));
    task_complete.Signal();
  }));
  task_complete.Wait();
}

//
// WaitUntilIdle tests
//

TEST_F(DispatcherTest, WaitUntilIdle) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
  WaitUntilIdle(dispatcher);
  ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
}

TEST_F(DispatcherTest, WaitUntilIdleWithDirectCall) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  // We shouldn't actually block on a dispatcher that doesn't have ALLOW_SYNC_CALLS set,
  // but this is just for synchronizing the test.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadBlock(local_ch_, dispatcher, &entered_callback, &complete_blocking_read));

  std::thread t1 = std::thread([&] {
    // Make the call not reentrant, so that the read will run immediately once the write happens.
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  });

  // Wait for the read callback to be called, it will block until we signal it to complete.
  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  // Start a thread that blocks until the dispatcher is idle.
  libsync::Completion wait_started;
  libsync::Completion wait_complete;
  std::thread t2 = std::thread([&] {
    wait_started.Signal();
    WaitUntilIdle(dispatcher);
    ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
    wait_complete.Signal();
  });

  ASSERT_OK(wait_started.Wait(zx::time::infinite()));
  ASSERT_FALSE(wait_complete.signaled());
  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  complete_blocking_read.Signal();

  // Dispatcher should be idle now.
  ASSERT_OK(wait_complete.Wait(zx::time::infinite()));

  t1.join();
  t2.join();
}

TEST_F(DispatcherTest, WaitUntilIdleWithAsyncLoop) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  // We shouldn't actually block on a dispatcher that doesn't have ALLOW_SYNC_CALLS set,
  // but this is just for synchronizing the test.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadBlock(local_ch_, dispatcher, &entered_callback, &complete_blocking_read));

  // Call is reentrant, so the read will be queued on the async loop.
  ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  // Wait for the read callback to be called, it will block until we signal it to complete.
  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  complete_blocking_read.Signal();
  WaitUntilIdle(dispatcher);
  ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
}

TEST_F(DispatcherTest, WaitUntilIdleCanceledRead) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  auto channel_read = std::make_unique<fdf::ChannelRead>(
      local_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_FALSE(true);  // This callback should never be called.
      });
  ASSERT_OK(channel_read->Begin(dispatcher));

  // Call is reentrant, so the read will be queued on the async loop.
  ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  ASSERT_OK(channel_read->Cancel());

  ASSERT_OK(fdf_testing_run_until_idle());
  WaitUntilIdle(dispatcher);
}

TEST_F(DispatcherTest, WaitUntilIdlePendingWait) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  ASSERT_OK(
      wait.Begin(async_dispatcher,
                 [](async_dispatcher_t* async_dispatcher, async::WaitOnce* wait, zx_status_t status,
                    const zx_packet_signal_t* signal) { ASSERT_FALSE(true); }));
  ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
  WaitUntilIdle(dispatcher);
}

TEST_F(DispatcherTest, WaitUntilIdleDelayedTask) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, CreateFakeDriver(), &dispatcher));

  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(dispatcher);
  ASSERT_NOT_NULL(async_dispatcher);

  async::TaskClosure task;
  task.set_handler([] { ASSERT_FALSE(true); });
  ASSERT_OK(task.PostForTime(async_dispatcher, zx::deadline_after(zx::sec(100))));

  ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
  WaitUntilIdle(dispatcher);

  ASSERT_OK(task.Cancel());  // Task should not be running yet.
}

TEST_F(DispatcherTest, WaitUntilIdleWithAsyncLoopMultipleThreads) {
  fdf_env_reset();

  constexpr uint32_t kNumThreads = 2;
  constexpr uint32_t kNumClients = 22;

  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, __func__, "",
                                           CreateFakeDriver(), &dispatcher));

  struct ReadClient {
    fdf::Channel channel;
    libsync::Completion entered_callback;
    libsync::Completion complete_blocking_read;
  };

  std::vector<ReadClient> local(kNumClients);
  std::vector<fdf::Channel> remote(kNumClients);

  for (uint32_t i = 0; i < kNumClients; i++) {
    auto channels = fdf::ChannelPair::Create(0);
    ASSERT_OK(channels.status_value());
    local[i].channel = std::move(channels->end0);
    remote[i] = std::move(channels->end1);
    ASSERT_NO_FATAL_FAILURE(RegisterAsyncReadBlock(local[i].channel.get(), dispatcher,
                                                   &local[i].entered_callback,
                                                   &local[i].complete_blocking_read));
  }

  fdf::Arena arena(nullptr);
  for (uint32_t i = 0; i < kNumClients; i++) {
    // Call is considered reentrant and will be queued on the async loop.
    auto write_status = remote[i].Write(0, arena, nullptr, 0, cpp20::span<zx_handle_t>());
    ASSERT_OK(write_status.status_value());
  }

  for (uint32_t i = 0; i < kNumThreads; i++) {
    StartAdditionalManagedThread();
  }

  ASSERT_OK(local[0].entered_callback.Wait(zx::time::infinite()));
  local[0].complete_blocking_read.Signal();

  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  // Allow all the read callbacks to complete.
  for (uint32_t i = 1; i < kNumClients; i++) {
    local[i].complete_blocking_read.Signal();
  }

  WaitUntilIdle(dispatcher);

  for (uint32_t i = 0; i < kNumClients; i++) {
    ASSERT_TRUE(local[i].complete_blocking_read.signaled());
  }
}

TEST_F(DispatcherTest, WaitUntilIdleMultipleDispatchers) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  fdf_dispatcher_t* dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher2));

  // We shouldn't actually block on a dispatcher that doesn't have ALLOW_SYNC_CALLS set,
  // but this is just for synchronizing the test.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadBlock(local_ch_, dispatcher, &entered_callback, &complete_blocking_read));

  // Call is reentrant, so the read will be queued on the async loop.
  ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  // Wait for the read callback to be called, it will block until we signal it to complete.
  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());
  ASSERT_TRUE(dispatcher2->GetDispatcher()->IsIdle());
  WaitUntilIdle(dispatcher2);

  complete_blocking_read.Signal();
  WaitUntilIdle(dispatcher);
  ASSERT_TRUE(dispatcher->GetDispatcher()->IsIdle());
}

TEST_F(DispatcherTest, SyncDispatcherCancelRequestDuringShutdown) {
  DispatcherShutdownObserver observer;

  const void* driver = CreateFakeDriver();
  constexpr std::string_view scheduler_role = "";

  driver_runtime::Dispatcher* dispatcher;
  {
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(0, "", scheduler_role,
                                                        observer.fdf_observer(), &dispatcher));
  }
  // Register a channel read that will be canceled by a posted task.
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      local_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_FALSE(true);  // This should never be called.
      });
  ASSERT_OK(channel_read->Begin(dispatcher->to_fdf_dispatcher()));

  libsync::Completion task_started;
  libsync::Completion dispatcher_shutdown_started;

  ASSERT_OK(async::PostTask(dispatcher->GetAsyncDispatcher(), [&] {
    task_started.Signal();
    ASSERT_OK(dispatcher_shutdown_started.Wait(zx::time::infinite()));
    ASSERT_OK(channel_read->Cancel());
  }));

  ASSERT_OK(task_started.Wait(zx::time::infinite()));

  // |Dispatcher::ShutdownAsync| will move the registered channel read into |shutdown_queue_|.
  dispatcher->ShutdownAsync();
  dispatcher_shutdown_started.Signal();

  ASSERT_OK(observer.WaitUntilShutdown());
  dispatcher->Destroy();
}

//
// Run/Quit tests
//

TEST_F(DispatcherTest, RunThenQuitAndRunAgain) {
  const void* driver = CreateFakeDriver();

  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateUnmanagedDispatcher(0, __func__, driver, &dispatcher));

  // Calls quit in 100ms
  std::atomic_bool ran = false;
  ASSERT_OK(async::PostTaskForTime(
      fdf_dispatcher_get_async_dispatcher(dispatcher),
      [&] {
        ran = true;
        fdf_testing_quit();
      },
      zx::deadline_after(zx::msec(100))));

  // We should hit our 1ms deadline before quit happens.
  ASSERT_EQ(ZX_ERR_TIMED_OUT, fdf_testing_run(zx::deadline_after(zx::msec(1)).get(), false));
  ASSERT_FALSE(ran);

  // This time quit task should run since we are not setting any deadline.
  ASSERT_EQ(ZX_ERR_CANCELED, fdf_testing_run(ZX_TIME_INFINITE, false));
  ASSERT_TRUE(ran);

  // Reset quit.
  fdf_testing_reset_quit();
  ran = false;

  // Calls quit in 100ms
  ASSERT_OK(async::PostTaskForTime(
      fdf_dispatcher_get_async_dispatcher(dispatcher),
      [&] {
        ran = true;
        fdf_testing_quit();
      },
      zx::deadline_after(zx::msec(100))));

  // We should hit our 1ms deadline again.
  ASSERT_EQ(ZX_ERR_TIMED_OUT, fdf_testing_run(zx::deadline_after(zx::msec(1)).get(), false));
  ASSERT_FALSE(ran);

  // Quit task should run since there is no deadline.
  ASSERT_EQ(ZX_ERR_CANCELED, fdf_testing_run(ZX_TIME_INFINITE, false));
  ASSERT_TRUE(ran);

  // Reset quit.
  fdf_testing_reset_quit();
}

//
// Misc tests
//

TEST_F(DispatcherTest, GetCurrentDispatcherNone) {
  ASSERT_NULL(fdf_dispatcher_get_current_dispatcher());
}

TEST_F(DispatcherTest, GetCurrentDispatcher) {
  const void* driver1 = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher1;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver1, &dispatcher1));

  const void* driver2 = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", driver2, &dispatcher2));

  // driver1 will wait on a message from driver2, then reply back.
  auto channel_read1 = std::make_unique<fdf::ChannelRead>(
      local_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);
        ASSERT_EQ(dispatcher1, fdf_dispatcher_get_current_dispatcher());
        // This reply will be reentrant and queued on the async loop.
        ASSERT_EQ(ZX_OK, fdf_channel_write(local_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
      });
  ASSERT_OK(channel_read1->Begin(dispatcher1));

  libsync::Completion got_reply;
  auto channel_read2 = std::make_unique<fdf::ChannelRead>(
      remote_ch_, 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        ASSERT_OK(status);
        ASSERT_EQ(dispatcher2, fdf_dispatcher_get_current_dispatcher());
        got_reply.Signal();
      });
  ASSERT_OK(channel_read2->Begin(dispatcher2));

  // Write from driver 2 to driver1.
  ASSERT_OK(async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher2), [&] {
    ASSERT_EQ(dispatcher2, fdf_dispatcher_get_current_dispatcher());
    // Non-reentrant write.
    ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  }));

  ASSERT_OK(got_reply.Wait(zx::time::infinite()));
  WaitUntilIdle(dispatcher2);
}

TEST_F(DispatcherTest, GetCurrentDispatcherShutdownCallback) {
  libsync::Completion shutdown_completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* shutdown_dispatcher) mutable {
    ASSERT_EQ(shutdown_dispatcher, fdf_dispatcher_get_current_dispatcher());
    shutdown_completion.Signal();
  };

  fdf::Dispatcher dispatcher;

  {
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    auto dispatcher_with_status = fdf::SynchronizedDispatcher::Create({}, "", shutdown_handler);
    ASSERT_FALSE(dispatcher_with_status.is_error());
    dispatcher = *std::move(dispatcher_with_status);
  }

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);

  // Registered, but not yet signaled.
  async_dispatcher_t* async_dispatcher = dispatcher.async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  libsync::Completion wait_complete;
  ASSERT_OK(wait.Begin(async_dispatcher, [&wait_complete, event = std::move(event)](
                                             async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    ASSERT_STATUS(status, ZX_ERR_CANCELED);
    ASSERT_EQ(dispatcher, fdf_dispatcher_get_current_dispatcher());
    wait_complete.Signal();
  }));

  // Shutdown the dispatcher, which should schedule cancellation of the channel read.
  dispatcher.ShutdownAsync();

  ASSERT_OK(wait_complete.Wait(zx::time::infinite()));
  shutdown_completion.Wait();
}

TEST_F(DispatcherTest, HasQueuedTasks) {
  fdf_dispatcher_t* dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &dispatcher));

  ASSERT_FALSE(dispatcher->GetDispatcher()->HasQueuedTasks());

  // We shouldn't actually block on a dispatcher that doesn't have ALLOW_SYNC_CALLS set,
  // but this is just for synchronizing the test.
  libsync::Completion entered_callback;
  libsync::Completion complete_blocking_read;
  ASSERT_NO_FATAL_FAILURE(
      RegisterAsyncReadBlock(local_ch_, dispatcher, &entered_callback, &complete_blocking_read));

  // Call is reentrant, so the read will be queued on the async loop.
  ASSERT_EQ(ZX_OK, fdf_channel_write(remote_ch_, 0, nullptr, nullptr, 0, nullptr, 0));
  ASSERT_FALSE(dispatcher->GetDispatcher()->IsIdle());

  // Wait for the read callback to be called, it will block until we signal it to complete.
  ASSERT_OK(entered_callback.Wait(zx::time::infinite()));

  libsync::Completion entered_task;
  ASSERT_OK(async::PostTask(dispatcher, [&] { entered_task.Signal(); }));
  ASSERT_TRUE(dispatcher->GetDispatcher()->HasQueuedTasks());

  complete_blocking_read.Signal();

  entered_task.Wait();
  ASSERT_FALSE(dispatcher->GetDispatcher()->HasQueuedTasks());

  WaitUntilIdle(dispatcher);
  ASSERT_FALSE(dispatcher->GetDispatcher()->HasQueuedTasks());
}

// Tests shutting down all the dispatchers owned by a driver.
TEST_F(DispatcherTest, ShutdownAllDriverDispatchers) {
  const void* fake_driver = CreateFakeDriver();
  const void* fake_driver2 = CreateFakeDriver();
  const std::string_view scheduler_role = "";

  constexpr uint32_t kNumDispatchers = 3;
  DispatcherShutdownObserver observers[kNumDispatchers];
  driver_runtime::Dispatcher* dispatchers[kNumDispatchers];

  for (uint32_t i = 0; i < kNumDispatchers; i++) {
    const void* driver = i == 0 ? fake_driver : fake_driver2;
    thread_context::PushDriver(driver, nullptr);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    ASSERT_EQ(ZX_OK, driver_runtime::Dispatcher::Create(
                         0, "", scheduler_role, observers[i].fdf_observer(), &dispatchers[i]));
  }

  // Shutdown the second driver, dispatchers[1] and dispatchers[2] should be shutdown.
  fdf_env::DriverShutdown driver2_shutdown;
  libsync::Completion driver2_shutdown_completion;
  ASSERT_OK(driver2_shutdown.Begin(fake_driver2, [&](const void* driver) {
    ASSERT_EQ(fake_driver2, driver);
    driver2_shutdown_completion.Signal();
  }));

  ASSERT_OK(observers[1].WaitUntilShutdown());
  ASSERT_OK(observers[2].WaitUntilShutdown());
  driver2_shutdown_completion.Wait();

  // Shutdown the first driver, dispatchers[0] should be shutdown.
  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver2_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));

  ASSERT_OK(observers[0].WaitUntilShutdown());
  driver_shutdown_completion.Wait();

  for (uint32_t i = 0; i < kNumDispatchers; i++) {
    dispatchers[i]->Destroy();
  }
}

TEST_F(DispatcherTest, DriverDestroysDispatcherShutdownByDriverHost) {
  zx::result<fdf::Dispatcher> dispatcher;

  libsync::Completion completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* shutdown_dispatcher) mutable {
    ASSERT_EQ(shutdown_dispatcher, dispatcher->get());
    dispatcher->reset();
    completion.Signal();
  };

  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  dispatcher = fdf::SynchronizedDispatcher::Create({}, "", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));

  completion.Wait();
  driver_shutdown_completion.Wait();
}

TEST_F(DispatcherTest, CannotCreateNewDispatcherDuringDriverShutdown) {
  libsync::Completion completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* shutdown_dispatcher) { completion.Signal(); };

  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  libsync::Completion task_started;
  libsync::Completion driver_shutting_down;
  ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&] {
    task_started.Signal();
    ASSERT_OK(driver_shutting_down.Wait(zx::time::infinite()));
    auto dispatcher =
        fdf::SynchronizedDispatcher::Create({}, "", [](fdf_dispatcher_t* dispatcher) {});
    // Creating a new dispatcher should fail, as the driver is currently shutting down.
    ASSERT_TRUE(dispatcher.is_error());
  }));
  ASSERT_OK(task_started.Wait(zx::time::infinite()));

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));

  driver_shutting_down.Signal();

  ASSERT_OK(completion.Wait(zx::time::infinite()));
  driver_shutdown_completion.Wait();
}

// Tests shutting down all dispatchers for a driver, but the dispatchers are already in a shutdown
// state.
TEST_F(DispatcherTest, ShutdownAllDispatchersAlreadyShutdown) {
  libsync::Completion completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* shutdown_dispatcher) { completion.Signal(); };

  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  dispatcher->ShutdownAsync();
  ASSERT_OK(completion.Wait(zx::time::infinite()));

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));
  driver_shutdown_completion.Wait();
}

// Tests shutting down all dispatchers for a driver, but the dispatcher is in the shutdown observer
// callback.
TEST_F(DispatcherTest, ShutdownAllDispatchersCurrentlyInShutdownCallback) {
  libsync::Completion entered_shutdown_handler;
  libsync::Completion complete_shutdown_handler;
  auto shutdown_handler = [&](fdf_dispatcher_t* shutdown_dispatcher) {
    entered_shutdown_handler.Signal();
    complete_shutdown_handler.Wait();
  };

  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  dispatcher->ShutdownAsync();
  entered_shutdown_handler.Wait();

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));

  // The dispatcher is still in the dispatcher shutdown handler.
  ASSERT_FALSE(driver_shutdown_completion.signaled());
  complete_shutdown_handler.Signal();
  driver_shutdown_completion.Wait();
}

TEST_F(DispatcherTest, DestroyAllDispatchers) {
  // Create drivers which leak their dispatchers.
  auto fake_driver = CreateFakeDriver();
  {
    thread_context::PushDriver(fake_driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    auto dispatcher =
        fdf::SynchronizedDispatcher::Create({}, "", [](fdf_dispatcher_t* dispatcher) {});
    ASSERT_FALSE(dispatcher.is_error());
    dispatcher->release();
  }

  auto fake_driver2 = CreateFakeDriver();
  {
    thread_context::PushDriver(fake_driver2);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });
    auto dispatcher2 =
        fdf::SynchronizedDispatcher::Create({}, "", [](fdf_dispatcher_t* dispatcher) {});
    ASSERT_FALSE(dispatcher2.is_error());
    dispatcher2->release();
  }

  // Driver host shuts down all drivers.
  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));
  driver_shutdown_completion.Wait();
  driver_shutdown_completion.Reset();

  ASSERT_OK(driver_shutdown.Begin(fake_driver2, [&](const void* driver) {
    ASSERT_EQ(fake_driver2, driver);
    driver_shutdown_completion.Signal();
  }));
  driver_shutdown_completion.Wait();

  // This will stop memory from leaking.
  fdf_env_destroy_all_dispatchers();
}

TEST_F(DispatcherTest, WaitUntilDispatchersDestroyed) {
  // No dispatchers, should immediately return.
  fdf_internal_wait_until_all_dispatchers_destroyed();

  constexpr uint32_t kNumDispatchers = 4;
  fdf_dispatcher_t* dispatchers[kNumDispatchers];

  for (uint32_t i = 0; i < kNumDispatchers; i++) {
    auto fake_driver = CreateFakeDriver();
    thread_context::PushDriver(fake_driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    auto dispatcher = fdf::SynchronizedDispatcher::Create(
        {}, "", [&](fdf_dispatcher_t* dispatcher) { fdf_dispatcher_destroy(dispatcher); });
    ASSERT_FALSE(dispatcher.is_error());
    dispatchers[i] = dispatcher->release();  // Destroyed in shutdown handler.
  }

  libsync::Completion thread_started;
  std::atomic_bool wait_complete = false;
  std::thread thread = std::thread([&]() {
    thread_started.Signal();
    fdf_internal_wait_until_all_dispatchers_destroyed();
    wait_complete = true;
  });

  thread_started.Wait();
  for (uint32_t i = 0; i < kNumDispatchers; i++) {
    // Not all dispatchers have been destroyed yet.
    ASSERT_FALSE(wait_complete);
    dispatchers[i]->GetDispatcher()->ShutdownAsync();
  }
  thread.join();
  ASSERT_TRUE(wait_complete);
}

// Tests waiting for all dispatchers to be destroyed when a driver shutdown
// observer is also registered.
TEST_F(DispatcherTest, WaitUntilDispatchersDestroyedHasDriverShutdownObserver) {
  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "", [&](fdf_dispatcher_t* dispatcher) { fdf_dispatcher_destroy(dispatcher); });
  ASSERT_FALSE(dispatcher.is_error());
  dispatcher->release();  // Destroyed in the shutdown handler.

  libsync::Completion thread_started;
  std::atomic_bool wait_complete = false;
  std::thread thread = std::thread([&]() {
    thread_started.Signal();
    fdf_internal_wait_until_all_dispatchers_destroyed();
    wait_complete = true;
  });

  thread_started.Wait();

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_completion.Signal();
  }));
  driver_shutdown_completion.Wait();

  thread.join();
  ASSERT_TRUE(wait_complete);
}

TEST_F(DispatcherTest, WaitUntilDispatchersDestroyedDuringDriverShutdownHandler) {
  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "", [&](fdf_dispatcher_t* dispatcher) { fdf_dispatcher_destroy(dispatcher); });
  ASSERT_FALSE(dispatcher.is_error());
  dispatcher->release();  // Destroyed in shutdown handler.

  // Block in the driver shutdown handler until we signal.
  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion driver_shutdown_started;
  libsync::Completion complete_driver_shutdown;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    ASSERT_EQ(fake_driver, driver);
    driver_shutdown_started.Signal();
    complete_driver_shutdown.Wait();
  }));

  driver_shutdown_started.Wait();

  // Start waiting for all dispatchers to be destroyed. This should not complete
  // until the shutdown handler completes.
  libsync::Completion thread_started;
  std::atomic_bool wait_complete = false;
  std::thread thread = std::thread([&]() {
    thread_started.Signal();
    fdf_internal_wait_until_all_dispatchers_destroyed();
    wait_complete = true;
  });

  thread_started.Wait();

  // Shutdown handler has not returned yet.
  ASSERT_FALSE(wait_complete);
  complete_driver_shutdown.Signal();

  thread.join();
  ASSERT_TRUE(wait_complete);
}

TEST_F(DispatcherTest, GetSequenceIdSynchronizedDispatcher) {
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher));
  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);

  fdf_dispatcher_t* fdf_dispatcher2;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(0, __func__, "", CreateFakeDriver(), &fdf_dispatcher2));
  async_dispatcher_t* async_dispatcher2 = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher2);

  async_sequence_id_t dispatcher_id;
  async_sequence_id_t dispatcher2_id;

  // Get the sequence id for the first dispatcher.
  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(async_dispatcher, [&] {
    const char* error = nullptr;
    ASSERT_EQ(ZX_ERR_INVALID_ARGS,
              async_get_sequence_id(async_dispatcher2, &dispatcher_id, &error));
    ASSERT_NOT_NULL(error);
    ASSERT_SUBSTR(error, "multiple driver dispatchers detected");
    error = nullptr;
    ASSERT_OK(async_get_sequence_id(async_dispatcher, &dispatcher_id, &error));
    ASSERT_NULL(error);
    task_completion.Signal();
  }));
  task_completion.Wait();

  // Get the sequence id for the second dispatcher.
  task_completion.Reset();
  ASSERT_OK(async::PostTask(async_dispatcher2, [&] {
    const char* error = nullptr;
    ASSERT_EQ(ZX_ERR_INVALID_ARGS,
              async_get_sequence_id(async_dispatcher, &dispatcher2_id, &error));
    ASSERT_NOT_NULL(error);
    ASSERT_SUBSTR(error, "multiple driver dispatchers detected");
    error = nullptr;
    ASSERT_OK(async_get_sequence_id(async_dispatcher2, &dispatcher2_id, &error));
    ASSERT_NULL(error);
    task_completion.Signal();
  }));
  task_completion.Wait();

  ASSERT_NE(dispatcher_id.value, dispatcher2_id.value);

  // Get the sequence id again for the first dispatcher.
  task_completion.Reset();
  ASSERT_OK(async::PostTask(async_dispatcher, [&] {
    async_sequence_id_t id;
    const char* error = nullptr;
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, async_get_sequence_id(async_dispatcher2, &id, &error));
    ASSERT_NOT_NULL(error);
    ASSERT_SUBSTR(error, "multiple driver dispatchers detected");
    error = nullptr;
    ASSERT_OK(async_get_sequence_id(async_dispatcher, &id, &error));
    ASSERT_NULL(error);
    ASSERT_EQ(id.value, dispatcher_id.value);
    task_completion.Signal();
  }));
  task_completion.Wait();

  // Get the sequence id from a non-managed thread.
  async_sequence_id_t id;
  const char* error = nullptr;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, async_get_sequence_id(async_dispatcher, &id, &error));
  ASSERT_NOT_NULL(error);
  ASSERT_SUBSTR(error, "not managed");
  error = nullptr;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, async_get_sequence_id(async_dispatcher2, &id, &error));
  ASSERT_NOT_NULL(error);
  ASSERT_SUBSTR(error, "not managed");
}

TEST_F(DispatcherTest, GetSequenceIdUnsynchronizedDispatcher) {
  fdf_dispatcher_t* fdf_dispatcher;
  ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_UNSYNCHRONIZED, __func__, "",
                                           CreateFakeDriver(), &fdf_dispatcher));
  async_dispatcher_t* async_dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);

  // Get the sequence id for the unsynchronized dispatcher.
  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(async_dispatcher, [&] {
    async_sequence_id_t id;
    const char* error = nullptr;
    ASSERT_EQ(ZX_ERR_WRONG_TYPE, async_get_sequence_id(async_dispatcher, &id, &error));
    ASSERT_NOT_NULL(error);
    ASSERT_SUBSTR(error, "UNSYNCHRONIZED");
    task_completion.Signal();
  }));
  task_completion.Wait();

  // Get the sequence id from a non-managed thread.
  async_sequence_id_t id;
  const char* error = nullptr;
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, async_get_sequence_id(async_dispatcher, &id, &error));
  ASSERT_NOT_NULL(error);
  ASSERT_SUBSTR(error, "UNSYNCHRONIZED");
}

//
// Error handling
//

// Tests that you cannot create an unsynchronized blocking dispatcher.
TEST_F(DispatcherTest, CreateUnsynchronizedAllowSyncCallsFails) {
  thread_context::PushDriver(CreateFakeDriver());
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  DispatcherShutdownObserver observer(false /* require_callback */);
  driver_runtime::Dispatcher* dispatcher;
  uint32_t options = FDF_DISPATCHER_OPTION_UNSYNCHRONIZED | FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS;
  ASSERT_NE(ZX_OK, driver_runtime::Dispatcher::Create(options, __func__, "",
                                                      observer.fdf_observer(), &dispatcher));
}

// Tests that you cannot create a dispatcher on a thread not managed by the driver runtime.
TEST_F(DispatcherTest, CreateDispatcherOnNonRuntimeThreadFails) {
  DispatcherShutdownObserver observer(false /* require_callback */);
  driver_runtime::Dispatcher* dispatcher;
  ASSERT_NE(ZX_OK, driver_runtime::Dispatcher::Create(0, __func__, "", observer.fdf_observer(),
                                                      &dispatcher));
}

// Tests that we don't spawn more threads than we need.
TEST_F(DispatcherTest, ExtraThreadIsReused) {
  {
    void* driver = reinterpret_cast<void*>(uintptr_t(1));
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    // a thread is created at startup that will be used for the first dispatcher
    ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads(), 1);

    // Create first dispatcher
    driver_runtime::Dispatcher* dispatcher;
    DispatcherShutdownObserver observer;
    ASSERT_OK(driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__,
                                                 "", observer.fdf_observer(), &dispatcher));
    ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads(), 1);

    dispatcher->ShutdownAsync();
    ASSERT_OK(observer.WaitUntilShutdown());
    dispatcher->Destroy();

    // Create second dispatcher
    DispatcherShutdownObserver observer2;
    ASSERT_OK(driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__,
                                                 "", observer2.fdf_observer(), &dispatcher));
    // Note that we are still at 1 thread.
    ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads(), 1);

    dispatcher->ShutdownAsync();
    ASSERT_OK(observer2.WaitUntilShutdown());
    dispatcher->Destroy();

    // Ideally we would let the initial thread go at this point, but that is challenging. A future
    // change may remedy this.
    ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads(), 1);
  }

  driver_runtime::GetDispatcherCoordinator().Reset();
  ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads(), 0);
}

TEST_F(DispatcherTest, MaximumThreads) {
  {
    void* driver = reinterpret_cast<void*>(uintptr_t(1));
    thread_context::PushDriver(driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    ASSERT_EQ(1, driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads());

    constexpr uint32_t kNumDispatchers = driver_runtime::ThreadPool::kDefaultThreadLimit;

    ASSERT_EQ(kNumDispatchers,
              driver_runtime::GetDispatcherCoordinator().default_thread_pool()->thread_limit());

    std::array<driver_runtime::Dispatcher*, kNumDispatchers> dispatchers;
    std::array<DispatcherShutdownObserver, kNumDispatchers> observers;
    for (uint32_t i = 0; i < kNumDispatchers; i++) {
      EXPECT_OK(driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__,
                                                   "", observers[i].fdf_observer(),
                                                   &dispatchers[i]));
      EXPECT_EQ(i + 1,
                driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads());
    }
    driver_runtime::Dispatcher* no_dispatcher = nullptr;
    ASSERT_EQ(ZX_ERR_NO_RESOURCES,
              driver_runtime::Dispatcher::Create(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__,
                                                 "", nullptr, &no_dispatcher));
    ASSERT_EQ(nullptr, no_dispatcher);

    ASSERT_EQ(kNumDispatchers,
              driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads());

    for (uint32_t i = 0; i < kNumDispatchers; i++) {
      dispatchers[i]->ShutdownAsync();
      ASSERT_OK(observers[i].WaitUntilShutdown());
      dispatchers[i]->Destroy();
    }
  }
}

TEST_F(DispatcherTest, GetDefaultThreadPoolSize) {
  ASSERT_EQ(driver_runtime::ThreadPool::kDefaultThreadLimit,
            driver_runtime::GetDispatcherCoordinator().default_thread_pool()->thread_limit());
}

TEST_F(DispatcherTest, SetDefaultThreadPoolSize) {
  ASSERT_OK(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->set_thread_limit(3));
  ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->thread_limit(), 3);
}

TEST_F(DispatcherTest, ThreadPoolSizeNeverGrowsPastMax) {
  static constexpr uint32_t kMaxThreads = 3;
  auto* thread_pool = driver_runtime::GetDispatcherCoordinator().default_thread_pool();
  ASSERT_EQ(thread_pool->set_thread_limit(kMaxThreads), ZX_OK);

  const void* driver = CreateFakeDriver();
  fdf_dispatcher_t* dispatcher;
  // Number of threads scales as we create dispatchers.
  for (uint32_t i = thread_pool->num_threads(); i < kMaxThreads; i++) {
    ASSERT_NO_FATAL_FAILURE(CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "",
                                             driver, &dispatcher));
    EXPECT_EQ(thread_pool->num_threads(), i);
  }

  // Creating one more doesn't scale us past the max.
  ASSERT_NO_FATAL_FAILURE(
      CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "", driver, &dispatcher));
  EXPECT_EQ(thread_pool->num_threads(), kMaxThreads);

  // Trying to change it to be lower than current number of threads errors out.
  ASSERT_STATUS(thread_pool->set_thread_limit(thread_pool->num_threads() - 1), ZX_ERR_OUT_OF_RANGE);

  // Changing the max one more doesn't scale us past the max.
  ASSERT_OK(thread_pool->set_thread_limit(kMaxThreads + 1));
  ASSERT_NO_FATAL_FAILURE(
      CreateDispatcher(FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS, __func__, "", driver, &dispatcher));
  EXPECT_EQ(thread_pool->num_threads(), kMaxThreads + 1);
}

// Tests shutting down and destroying multiple dispatchers concurrently.
TEST_F(DispatcherTest, ConcurrentDispatcherDestroy) {
  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  // Synchronize the dispatcher shutdown handlers to return at the same time,
  // so that |DispatcherCoordinator::NotifyShutdown| is more likely to happen concurrently.
  fbl::Mutex lock;
  bool dispatcher_shutdown = false;
  fbl::ConditionVariable all_dispatchers_shutdown;

  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) {
    fdf_dispatcher_destroy(dispatcher);

    fbl::AutoLock al(&lock);
    // IF the other dispatcher has shutdown, we should signal them to wake up.
    if (dispatcher_shutdown) {
      all_dispatchers_shutdown.Broadcast();
    } else {
      // Block until the other dispatcher completes shutdown.
      dispatcher_shutdown = true;
      all_dispatchers_shutdown.Wait(&lock);
    }
  };

  auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto dispatcher2 = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "", destructed_handler);
  ASSERT_FALSE(dispatcher2.is_error());

  // The dispatchers will be destroyed in their shutdown handlers.
  dispatcher->release();
  dispatcher2->release();

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion completion;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) { completion.Signal(); }));
  completion.Wait();

  // Wait for the driver to be removed from the dispatcher coordinator's |driver_state_| map as
  // |Reset| expects it to be empty.
  fdf_internal_wait_until_all_dispatchers_destroyed();
}

// Tests that the sequence id retrieved in the driver shutdown callback
// matches that of the initial dispatcher.
TEST_F(DispatcherTest, ShutdownCallbackSequenceId) {
  auto fake_driver = CreateFakeDriver();

  async_sequence_id_t initial_dispatcher_id;

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", [](fdf_dispatcher_t* dispatcher) {});

  // We will create a second dispatcher while running on the initial dispatcher.
  fdf::Dispatcher additional_dispatcher;

  libsync::Completion completion;
  ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&] {
    // This needs to be retrieved when running on the dispatcher thread.
    const char* error = nullptr;
    ASSERT_OK(async_get_sequence_id(dispatcher->get(), &initial_dispatcher_id, &error));
    ASSERT_NULL(error);

    auto result = fdf::SynchronizedDispatcher::Create({}, "", [&](fdf_dispatcher_t* dispatcher) {});
    ASSERT_FALSE(result.is_error());
    additional_dispatcher = std::move(*result);

    completion.Signal();
  }));

  completion.Wait();

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion shutdown;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    async_sequence_id_t shutdown_id;
    const char* error = nullptr;
    ASSERT_OK(async_get_sequence_id(dispatcher->get(), &shutdown_id, &error));
    ASSERT_NULL(error);
    ASSERT_EQ(shutdown_id.value, initial_dispatcher_id.value);
    shutdown.Signal();
  }));

  shutdown.Wait();
}

// Tests that the outgoing directory can be destructed on driver shutdown.
TEST_F(DispatcherTest, OutgoingDirectoryDestructionOnShutdown) {
  auto fake_driver = CreateFakeDriver();

  std::shared_ptr<fdf::OutgoingDirectory> outgoing;

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", [](fdf_dispatcher_t* dispatcher) {});

  // We will create a second dispatcher while running on the initial dispatcher.
  fdf::Dispatcher additional_dispatcher;

  libsync::Completion completion;
  ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&] {
    outgoing =
        std::make_shared<fdf::OutgoingDirectory>(fdf::OutgoingDirectory::Create(dispatcher->get()));

    auto result = fdf::SynchronizedDispatcher::Create({}, "", [&](fdf_dispatcher_t* dispatcher) {});
    ASSERT_FALSE(result.is_error());
    additional_dispatcher = std::move(*result);

    completion.Signal();
  }));

  completion.Wait();

  fdf_env::DriverShutdown driver_shutdown;
  libsync::Completion shutdown;
  ASSERT_OK(driver_shutdown.Begin(fake_driver, [&](const void* driver) {
    // The outgoing directory destructor will check that we are running on the
    // initial dispatcher's thread.
    outgoing.reset();
    shutdown.Signal();
  }));

  shutdown.Wait();
}

TEST_F(DispatcherTest, SynchronizedDispatcherWrapper) {
  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  {
    libsync::Completion completion;
    auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { completion.Signal(); };
    auto dispatcher = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler);
    ASSERT_FALSE(dispatcher.is_error());
    auto options = dispatcher->options();
    ASSERT_TRUE(options.has_value());
    ASSERT_EQ(*options, FDF_DISPATCHER_OPTION_SYNCHRONIZED);

    fdf::SynchronizedDispatcher dispatcher2 = *std::move(dispatcher);
    dispatcher2.ShutdownAsync();
    completion.Wait();
  }
  {
    libsync::Completion completion;
    auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { completion.Signal(); };
    auto blocking_dispatcher = fdf::SynchronizedDispatcher::Create(
        fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "", destructed_handler);
    ASSERT_FALSE(blocking_dispatcher.is_error());
    auto options = blocking_dispatcher->options();
    ASSERT_TRUE(options.has_value());
    ASSERT_EQ(*options,
              FDF_DISPATCHER_OPTION_SYNCHRONIZED | FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS);
    blocking_dispatcher->ShutdownAsync();
    completion.Wait();
  }
}

TEST_F(DispatcherTest, UnsynchronizedDispatcherWrapper) {
  auto fake_driver = CreateFakeDriver();
  thread_context::PushDriver(fake_driver);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  {
    libsync::Completion completion;
    auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { completion.Signal(); };
    auto dispatcher = fdf::UnsynchronizedDispatcher::Create({}, "", destructed_handler);
    ASSERT_FALSE(dispatcher.is_error());
    auto options = dispatcher->options();
    ASSERT_TRUE(options.has_value());
    ASSERT_EQ(*options, FDF_DISPATCHER_OPTION_UNSYNCHRONIZED);

    fdf::UnsynchronizedDispatcher dispatcher2 = *std::move(dispatcher);
    dispatcher2.ShutdownAsync();
    completion.Wait();
  }
}

TEST_F(DispatcherTest, SetDefaultDispatcher) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  ASSERT_OK(fdf_testing_set_default_dispatcher(dispatcher->get()));
  ASSERT_EQ(fdf_dispatcher_get_current_dispatcher(), dispatcher->get());

  // This thread has a default dispatcher, so we should be able to create
  // a dispatcher without using the env library.
  libsync::Completion shutdown_completion2;
  auto destructed_handler2 = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion2.Signal(); };
  auto dispatcher2 = fdf::SynchronizedDispatcher::Create({}, "", destructed_handler2);
  ASSERT_FALSE(dispatcher2.is_error());

  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(dispatcher2->async_dispatcher(), [&] {
    // We are running on a managed thread.
    ASSERT_NOT_OK(fdf_testing_set_default_dispatcher(dispatcher->get()));
    ASSERT_EQ(fdf_dispatcher_get_current_dispatcher(), dispatcher2->get());
    task_completion.Signal();
  }));
  task_completion.Wait();

  ASSERT_EQ(fdf_dispatcher_get_current_dispatcher(), dispatcher->get());

  dispatcher->ShutdownAsync();
  dispatcher2->ShutdownAsync();
  shutdown_completion.Wait();
  shutdown_completion2.Wait();

  ASSERT_OK(fdf_testing_set_default_dispatcher(nullptr));
  // A default dispatcher has not been set, so creating a dispatcher should fail.
  auto dispatcher3 =
      fdf::SynchronizedDispatcher::Create({}, "", [&](fdf_dispatcher_t* dispatcher) {});
  ASSERT_TRUE(dispatcher3.is_error());
}

// Tests that a delayed task cannot be queued after the dispatcher is shutdown.
TEST_F(DispatcherTest, QueueDelayedTaskAfterShutdown) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  // Choose a valid delay value, zx::time::infinite() is not allowed.
  constexpr zx::duration kDelay = zx::sec(1);
  ASSERT_EQ(ZX_ERR_BAD_STATE, async::PostDelayedTask(
                                  dispatcher->async_dispatcher(),
                                  [] {
                                    // This task should never run.
                                    ASSERT_FALSE(true);
                                  },
                                  kDelay));
}

// This test case was added as a regression test for https://fxbug.dev/508319815. It ensures that
// dispatchers can be safely created and asynchronously destroyed from multiple threads.
TEST_F(DispatcherTest, AddDispatcherRace) {
  constexpr uint32_t kNumThreads = 8;
  constexpr uint32_t kNumIterations = 100;

  std::array<std::thread, kNumThreads> threads;
  for (uint32_t i = 0; i < kNumThreads; ++i) {
    // CreateFakeDriver is not thread-safe, so grab the driver from the main thread.
    threads[i] = std::thread([driver = CreateFakeDriver()]() {
      for (uint32_t j = 0; j < kNumIterations; ++j) {
        thread_context::PushDriver(driver);
        auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

        zx::result dispatcher = fdf::SynchronizedDispatcher::Create(
            {}, "", [&](fdf_dispatcher_t* dispatcher) { dispatcher->GetDispatcher()->Destroy(); },
            "role");
        ASSERT_FALSE(dispatcher.is_error());

        // The dispatcher is destroyed in the shutdown handler.
        fdf_dispatcher_t* d = dispatcher->release();
        d->GetDispatcher()->ShutdownAsync();
      }
    });
  }

  for (auto& t : threads) {
    t.join();
  }
  fdf_internal_wait_until_all_dispatchers_destroyed();
}

static libsync::Completion* g_suspend_completion = nullptr;

TEST_F(DispatcherTest, SuspendResume) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  // We need to use DispatcherBuilder to set the owner.
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto async_dispatcher = dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  // Start a managed thread to run the loop, avoiding fdf_testing_run.
  StartAdditionalManagedThread();

  // Suspend the driver.
  libsync::Completion suspend_completion;
  g_suspend_completion = &suspend_completion;

  fdf_env_suspend_completer_t completer = {
      .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
  };

  fdf_env_driver_suspend(fake_driver, &completer);
  suspend_completion.Wait();

  // Post a task that should be deferred.
  std::atomic_bool ran = false;
  ASSERT_OK(async::PostTask(async_dispatcher, [&] { ran = true; }));

  // Wait a bit to ensure it doesn't run.
  zx::nanosleep(zx::deadline_after(zx::msec(50)));
  ASSERT_FALSE(ran);

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  // Post another task with a completion to wait for it.
  libsync::Completion task_done;
  ASSERT_OK(async::PostTask(async_dispatcher, [&] { task_done.Signal(); }));

  ASSERT_OK(task_done.Wait(zx::time::infinite()));
  ASSERT_TRUE(ran);  // The first task should have run too!

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  g_suspend_completion = nullptr;
}

TEST_F(DispatcherTest, AlwaysOnDispatcher) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  fdf_dispatcher_t* always_on_dispatcher =
      fdf_dispatcher_get_always_on_dispatcher(dispatcher->get());
  ASSERT_NOT_NULL(always_on_dispatcher);

  // Calling fdf_dispatcher_get_always_on_dispatcher on a veneer should be safe
  // and return the veneer itself.
  fdf_dispatcher_t* always_on_dispatcher2 =
      fdf_dispatcher_get_always_on_dispatcher(always_on_dispatcher);
  EXPECT_EQ(always_on_dispatcher, always_on_dispatcher2);

  async_dispatcher_t* always_on_async = fdf_dispatcher_get_async_dispatcher(always_on_dispatcher);
  ASSERT_NOT_NULL(always_on_async);

  StartAdditionalManagedThread();

  // Suspend the driver.
  libsync::Completion suspend_completion;
  g_suspend_completion = &suspend_completion;

  fdf_env_suspend_completer_t completer = {
      .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
  };

  fdf_env_driver_suspend(fake_driver, &completer);
  suspend_completion.Wait();

  // Post a task to the always-on dispatcher. It should run even when suspended.
  libsync::Completion task_done;
  ASSERT_OK(async::PostTask(always_on_async, [&] { task_done.Signal(); }));

  ASSERT_OK(task_done.Wait(zx::time::infinite()));

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  g_suspend_completion = nullptr;
}

TEST_F(DispatcherTest, AlwaysOnDispatcherGetCurrentDispatcher) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  fdf_dispatcher_t* always_on_dispatcher =
      fdf_dispatcher_get_always_on_dispatcher(dispatcher->get());
  ASSERT_NOT_NULL(always_on_dispatcher);

  async_dispatcher_t* always_on_async = fdf_dispatcher_get_async_dispatcher(always_on_dispatcher);
  ASSERT_NOT_NULL(always_on_async);

  StartAdditionalManagedThread();

  // Post a task to the always-on dispatcher.
  libsync::Completion task_done;
  ASSERT_OK(async::PostTask(always_on_async, [&] {
    // Inside the always-on task, fdf_dispatcher_get_current_dispatcher()
    // should return the parent dispatcher, not the veneer.
    EXPECT_EQ(fdf_dispatcher_get_current_dispatcher(), dispatcher->get());
    task_done.Signal();
  }));

  ASSERT_OK(task_done.Wait(zx::time::infinite()));

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();
}

TEST_F(DispatcherTest, WakeVectorTriggersResume) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto async_dispatcher = dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Register the event as a wake vector.
  ASSERT_OK(fdf_dispatcher_register_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  // Register a resume requester.
  static libsync::Completion* g_resume_requested = nullptr;
  libsync::Completion resume_requested;
  g_resume_requested = &resume_requested;

  fdf_env_resume_requester_t requester = {
      .handler = [](fdf_env_resume_requester_t* r) -> int {
        g_resume_requested->Signal();
        return 0;
      },
  };
  fdf_env_register_resume_requester(fake_driver, &requester);

  StartAdditionalManagedThread();

  // Suspend the driver.
  libsync::Completion suspend_completion;
  g_suspend_completion = &suspend_completion;

  fdf_env_suspend_completer_t completer = {
      .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
  };

  fdf_env_driver_suspend(fake_driver, &completer);
  suspend_completion.Wait();

  // Post a wait on the event.
  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  std::atomic_bool wait_triggered = false;
  ASSERT_OK(wait.Begin(async_dispatcher,
                       [&](async_dispatcher_t* d, async::WaitOnce* w, zx_status_t status,
                           const zx_packet_signal_t* signal) { wait_triggered = true; }));

  // Signal the event. This should trigger the wake vector and call the resume requester.
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));

  // Verify that the resume requester was called.
  ASSERT_OK(resume_requested.Wait(zx::time::infinite()));

  // Verify that the wait has NOT triggered yet (since we are suspended).
  zx::nanosleep(zx::deadline_after(zx::msec(50)));
  ASSERT_FALSE(wait_triggered);

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  // Verify that the wait triggers after resume.
  libsync::Completion task_done;
  ASSERT_OK(async::PostTask(async_dispatcher, [&] { task_done.Signal(); }));
  ASSERT_OK(task_done.Wait(zx::time::infinite()));

  ASSERT_TRUE(wait_triggered);

  // Unregister wake vector.
  ASSERT_OK(
      fdf_dispatcher_unregister_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  g_suspend_completion = nullptr;
  g_resume_requested = nullptr;
}

TEST_F(DispatcherTest, WakeVectorRegistrationConstraints) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  StartAdditionalManagedThread();

  // Suspend the driver.
  libsync::Completion suspend_completion;
  g_suspend_completion = &suspend_completion;

  fdf_env_suspend_completer_t completer = {
      .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
  };

  fdf_env_driver_suspend(fake_driver, &completer);
  suspend_completion.Wait();

  // Try to register a wake vector while suspended. Should fail.
  ASSERT_EQ(ZX_ERR_BAD_STATE,
            fdf_dispatcher_register_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  // Register wake vector. Should succeed.
  ASSERT_OK(fdf_dispatcher_register_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  // Suspend again.
  suspend_completion.Reset();
  fdf_env_driver_suspend(fake_driver, &completer);
  suspend_completion.Wait();

  // Try to unregister wake vector while suspended. Should succeed.
  ASSERT_OK(
      fdf_dispatcher_unregister_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  g_suspend_completion = nullptr;
}

TEST_F(DispatcherTest, TimersDuringSuspend) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto async_dispatcher = dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  fdf_dispatcher_t* always_on_dispatcher =
      fdf_dispatcher_get_always_on_dispatcher(dispatcher->get());
  ASSERT_NOT_NULL(always_on_dispatcher);

  async_dispatcher_t* always_on_async = fdf_dispatcher_get_async_dispatcher(always_on_dispatcher);
  ASSERT_NOT_NULL(always_on_async);

  StartAdditionalManagedThread();

  // Post a normal delayed task.
  std::atomic_bool normal_ran = false;
  ASSERT_OK(async::PostDelayedTask(async_dispatcher, [&] { normal_ran = true; }, zx::msec(10)));

  // Post an always-on delayed task.
  std::atomic_bool always_on_ran = false;
  ASSERT_OK(async::PostDelayedTask(always_on_async, [&] { always_on_ran = true; }, zx::msec(10)));

  // Suspend the driver.
  libsync::Completion suspend_completion;
  g_suspend_completion = &suspend_completion;

  fdf_env_suspend_completer_t completer = {
      .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
  };

  fdf_env_driver_suspend(fake_driver, &completer);
  suspend_completion.Wait();

  // Wait enough time for the delays to expire.
  zx::nanosleep(zx::deadline_after(zx::msec(50)));

  // Verify that the always-on task ran.
  ASSERT_TRUE(always_on_ran);

  // Verify that the normal task did NOT run.
  ASSERT_FALSE(normal_ran);

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  // Verify that the normal task triggers after resume.
  libsync::Completion task_done;
  ASSERT_OK(async::PostTask(async_dispatcher, [&] { task_done.Signal(); }));
  ASSERT_OK(task_done.Wait(zx::time::infinite()));

  ASSERT_TRUE(normal_ran);

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  g_suspend_completion = nullptr;
}

TEST_F(DispatcherTest, SuspendResumeMultipleCycles) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto async_dispatcher = dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  StartAdditionalManagedThread();

  constexpr int kCycles = 3;
  for (int i = 0; i < kCycles; i++) {
    std::atomic_bool ran = false;

    // Suspend the driver.
    libsync::Completion suspend_completion;
    g_suspend_completion = &suspend_completion;

    fdf_env_suspend_completer_t completer = {
        .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
    };

    fdf_env_driver_suspend(fake_driver, &completer);
    suspend_completion.Wait();

    // Post task AFTER suspend.
    ASSERT_OK(async::PostTask(async_dispatcher, [&] { ran = true; }));

    // Verify it doesn't run while suspended.
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
    ASSERT_FALSE(ran);

    // Now resume.
    fdf_env_driver_resume(fake_driver);

    // Verify it runs after resume.
    libsync::Completion task_done;
    ASSERT_OK(async::PostTask(async_dispatcher, [&] { task_done.Signal(); }));
    ASSERT_OK(task_done.Wait(zx::time::infinite()));

    ASSERT_TRUE(ran);

    g_suspend_completion = nullptr;
  }

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();
}

TEST_F(DispatcherTest, WakeVectorTriggeredBeforeSuspend) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto destructed_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", destructed_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto async_dispatcher = dispatcher->async_dispatcher();
  ASSERT_NOT_NULL(async_dispatcher);

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Register the event as a wake vector.
  ASSERT_OK(fdf_dispatcher_register_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  // Register a resume requester.
  static libsync::Completion* g_resume_requested = nullptr;
  libsync::Completion resume_requested;
  g_resume_requested = &resume_requested;

  fdf_env_resume_requester_t requester = {
      .handler = [](fdf_env_resume_requester_t* r) -> int {
        g_resume_requested->Signal();
        return 0;
      },
  };
  fdf_env_register_resume_requester(fake_driver, &requester);

  StartAdditionalManagedThread();

  libsync::Completion callback_started;
  libsync::Completion allow_callback_to_finish;

  // Post a wait on the event.
  async::WaitOnce wait(event.get(), ZX_USER_SIGNAL_0);
  std::atomic_bool wait_triggered = false;
  ASSERT_OK(wait.Begin(async_dispatcher, [&](async_dispatcher_t* d, async::WaitOnce* w,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
    callback_started.Signal();
    allow_callback_to_finish.Wait();
    wait_triggered = true;
  }));

  // Signal the event.
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));

  // Wait for the callback to start executing.
  ASSERT_OK(callback_started.Wait(zx::time::infinite()));

  // Now suspend. The callback is executing, so it should trigger resume.
  libsync::Completion suspend_completion;
  g_suspend_completion = &suspend_completion;

  fdf_env_suspend_completer_t completer = {
      .handler = [](fdf_env_suspend_completer_t* c) { g_suspend_completion->Signal(); },
  };

  fdf_env_driver_suspend(fake_driver, &completer);

  // Verify that the resume requester was called (because it found a running wake vector).
  ASSERT_OK(resume_requested.Wait(zx::time::infinite()));

  // The suspend operation is waiting for the callback to finish.
  ASSERT_FALSE(suspend_completion.signaled());

  // Let the callback finish.
  allow_callback_to_finish.Signal();

  // Now suspend should complete.
  suspend_completion.Wait();

  // Verify that the wait triggered.
  ASSERT_TRUE(wait_triggered);

  // Now resume.
  fdf_env_driver_resume(fake_driver);

  // Unregister wake vector.
  ASSERT_OK(
      fdf_dispatcher_unregister_wake_vector(dispatcher->get(), event.get(), ZX_USER_SIGNAL_0));

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();

  g_suspend_completion = nullptr;
  g_resume_requested = nullptr;
}

TEST_F(DispatcherTest, WakeVectorAndAlwaysOnCppApi) {
  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "test-dispatcher", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  // Test always on dispatcher API
  fdf::UnownedDispatcher always_on = dispatcher->GetAlwaysOnDispatcher();
  ASSERT_NOT_NULL(always_on->get());

  // Test wake vector registration API
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  {
    auto reg = fdf::WakeVectorRegistration::Create(*dispatcher, zx::unowned_handle(event.get()),
                                                   ZX_USER_SIGNAL_0);
    ASSERT_TRUE(reg.is_ok());
    ASSERT_TRUE(*reg);  // operator bool
  }  // reg goes out of scope and unregisters

  // Trying to unregister it again should fail with ZX_ERR_NOT_FOUND if it was successfully
  // unregistered by RAII
  ASSERT_EQ(ZX_ERR_NOT_FOUND, fdf_dispatcher_unregister_wake_vector(dispatcher->get(), event.get(),
                                                                    ZX_USER_SIGNAL_0));

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();
}
