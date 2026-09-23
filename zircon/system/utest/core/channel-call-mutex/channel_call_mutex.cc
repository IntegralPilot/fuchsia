// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/sync/mutex.h>
#include <lib/zx/channel.h>
#include <lib/zx/profile.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/syscalls/resource.h>

#include <thread>

#include <zxtest/zxtest.h>

namespace {

zx::result<> SetDeadlineProfile() {
  zx::resource resource;
  zx_status_t status = zx::resource::create(*standalone::GetSystemResource(), ZX_RSRC_KIND_SYSTEM,
                                            ZX_RSRC_SYSTEM_PROFILE_BASE, 1, nullptr, 0, &resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  zx_profile_info_t info = {};
  info.flags = ZX_PROFILE_INFO_FLAG_DEADLINE;
  info.deadline_params = {
      .capacity = ZX_MSEC(8),
      .relative_deadline = ZX_MSEC(16),
      .period = ZX_MSEC(16),
  };
  zx::profile profile;
  status = zx::profile::create(resource, 0, &info, &profile);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = zx::thread::self()->set_profile(profile, 0);
  return zx::make_result(status);
}

TEST(ChannelCallMutexTest, MutexHeldAcrossCallChain) {
  sync_mutex_t mutex;
  zx::channel client1, server1;
  zx::channel client2, server2;

  constexpr zx_txid_t kTxid1 = 1;
  constexpr zx_txid_t kTxid2 = 2;

  ASSERT_OK(zx::channel::create(0, &client1, &server1));
  ASSERT_OK(zx::channel::create(0, &client2, &server2));

  // Thread 1: Sets a deadline profile on the thread, then Locks a mutex, and makes a
  // zx::channel::call to the second thread.
  std::thread t1([&] {
    ASSERT_OK(SetDeadlineProfile());

    sync_mutex_lock(&mutex);

    // Make a call to Thread 2.
    zx_txid_t bytes[2] = {kTxid1, 0};
    zx_channel_call_args_t args = {};
    args.wr_bytes = bytes;
    args.wr_num_bytes = sizeof(bytes);
    args.rd_bytes = bytes;
    args.rd_num_bytes = sizeof(bytes);
    uint32_t actual_bytes, actual_handles;

    // Block until Thread 2 replies.
    ASSERT_OK(client1.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));

    sync_mutex_unlock(&mutex);
  });

  // Thread 2: Receives call from Thread 1, then makes a zx::channel::call to the third thread.
  std::thread t2([&] {
    ASSERT_OK(server1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));

    // Make a call to Thread 3.
    zx_txid_t bytes[2] = {kTxid2, 0};
    zx_channel_call_args_t args = {};
    args.wr_bytes = bytes;
    args.wr_num_bytes = sizeof(bytes);
    args.rd_bytes = bytes;
    args.rd_num_bytes = sizeof(bytes);
    uint32_t actual_bytes, actual_handles;

    // Block until Thread 3 replies.
    ASSERT_OK(client2.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
    ASSERT_OK(server1.wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(server1.write(0, &bytes, sizeof(bytes), nullptr, 0));
  });

  // Thread 3: Receives call from Thread 2, then attempts to lock the mutex held by the first
  // thread.
  std::thread t3([&] {
    ASSERT_OK(server2.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));

    // Attempt to lock the mutex held by Thread 1.
    // This will fail after a 100 ms timeout, in order to finish the test.
    ASSERT_EQ(sync_mutex_timedlock(&mutex, zx_deadline_after(ZX_MSEC(100))), ZX_ERR_TIMED_OUT);

    zx_txid_t bytes[2] = {};
    uint32_t actual_bytes, actual_handles;

    ASSERT_OK(server2.read(0, &bytes, nullptr, sizeof(bytes), 0, &actual_bytes, &actual_handles));
    ASSERT_OK(server2.wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(server2.write(0, &bytes, sizeof(bytes), nullptr, 0));
  });

  t3.join();
  t2.join();
  t1.join();
}

}  // namespace
