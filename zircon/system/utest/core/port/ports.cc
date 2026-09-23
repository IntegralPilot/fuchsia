// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <lib/zx/event.h>
#include <lib/zx/exception.h>
#include <lib/zx/job.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/port.h>
#include <zircon/types.h>

#include <atomic>
#include <barrier>
#include <cstdio>
#include <iterator>
#include <latch>
#include <string>
#include <thread>

#include <fbl/algorithm.h>
#include <mini-process/mini-process.h>
#include <zxtest/zxtest.h>

namespace {

TEST(PortTest, QueueNullPtrReturnsInvalidArgs) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  EXPECT_EQ(port.queue(nullptr), ZX_ERR_INVALID_ARGS);
}

TEST(PortTest, QueueWaitVerifyUserPacket) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  constexpr zx_port_packet_t kPortUserPacket = {
      12ull,
      ZX_PKT_TYPE_USER + 5u,  // kernel overrides the |type|.
      -3,
      {{}}};

  zx_port_packet_t out = {};

  ASSERT_OK(port.queue(&kPortUserPacket));

  ASSERT_OK(port.wait(zx::time::infinite(), &out));

  EXPECT_EQ(out.key, 12u);
  EXPECT_EQ(out.type, ZX_PKT_TYPE_USER);
  EXPECT_EQ(out.status, -3);

  EXPECT_EQ(memcmp(&kPortUserPacket.user, &out.user, sizeof(zx_port_packet_t::user)), 0);
}

TEST(PortTest, PortTimeout) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx_port_packet_t packet = {};

  EXPECT_EQ(port.wait(zx::deadline_after(zx::nsec(1)), &packet), ZX_ERR_TIMED_OUT);
}

TEST(PortTest, QueueAndClose) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  constexpr zx_port_packet_t kPortUserPacket = {1ull, ZX_PKT_TYPE_USER, 0, {{}}};

  EXPECT_OK(port.queue(&kPortUserPacket));
}

TEST(PortTest, AsyncWaitChannelTimedOut) {
  constexpr uint64_t kEventKey = 6567;

  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx::channel ch[2];
  ASSERT_OK(zx::channel::create(0u, &ch[0], &ch[1]));

  zx_port_packet_t out = {};
  ASSERT_OK(ch[1].wait_async(port, kEventKey, ZX_CHANNEL_READABLE, 0));

  EXPECT_EQ(port.wait(zx::deadline_after(zx::usec(200)), &out), ZX_ERR_TIMED_OUT);
}

TEST(PortTest, AsyncWaitChannel) {
  constexpr uint64_t kEventKey = 6567;

  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx::channel ch[2];
  ASSERT_OK(zx::channel::create(0u, &ch[0], &ch[1]));

  zx_port_packet_t out = {};
  ASSERT_OK(ch[1].wait_async(port, kEventKey, ZX_CHANNEL_READABLE, 0));

  EXPECT_EQ(port.wait(zx::deadline_after(zx::usec(200)), &out), ZX_ERR_TIMED_OUT);

  EXPECT_OK(ch[0].write(0u, "here", 4, nullptr, 0u));

  EXPECT_OK(port.wait(zx::time::infinite(), &out));

  EXPECT_EQ(out.key, kEventKey);
  EXPECT_EQ(out.type, ZX_PKT_TYPE_SIGNAL_ONE);
  EXPECT_EQ(out.signal.observed, ZX_CHANNEL_WRITABLE | ZX_CHANNEL_READABLE);
  EXPECT_EQ(out.signal.trigger, ZX_CHANNEL_READABLE);
  EXPECT_EQ(out.signal.count, 1u);

  EXPECT_EQ(ch[1].read(ZX_CHANNEL_READ_MAY_DISCARD, nullptr, nullptr, 0u, 0, nullptr, nullptr),
            ZX_ERR_BUFFER_TOO_SMALL);

  zx_port_packet_t out1 = {};
  EXPECT_EQ(port.wait(zx::deadline_after(zx::usec(200)), &out1), ZX_ERR_TIMED_OUT);

  EXPECT_OK(ch[1].wait_async(port, kEventKey, ZX_CHANNEL_READABLE, 0));
}

TEST(PortTest, WaitAsyncBootTimestamp) {
  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  // 1. Verify mutually exclusive timestamp flags return ZX_ERR_INVALID_ARGS.
  EXPECT_EQ(event.wait_async(port, 1u, ZX_EVENT_SIGNALED,
                             ZX_WAIT_ASYNC_TIMESTAMP | ZX_WAIT_ASYNC_BOOT_TIMESTAMP),
            ZX_ERR_INVALID_ARGS);

  // 2. Verify that without timestamp options (options = 0), packet.signal.timestamp is 0.
  ASSERT_OK(event.wait_async(port, 1u, ZX_EVENT_SIGNALED, 0u));
  ASSERT_OK(event.signal(0u, ZX_EVENT_SIGNALED));

  zx_port_packet_t packet = {};
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));

  EXPECT_EQ(packet.key, 1u);
  EXPECT_EQ(packet.type, ZX_PKT_TYPE_SIGNAL_ONE);
  EXPECT_EQ(packet.signal.trigger, ZX_EVENT_SIGNALED);
  EXPECT_EQ(packet.signal.timestamp, 0u);

  // Reset the signal for the next wait.
  ASSERT_OK(event.signal(ZX_EVENT_SIGNALED, 0u));

  // 3. Verify that ZX_WAIT_ASYNC_BOOT_TIMESTAMP returns a bounded boot timeline timestamp.
  ASSERT_OK(event.wait_async(port, 2u, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_BOOT_TIMESTAMP));

  const zx::time_boot before = zx::clock::get_boot();
  ASSERT_OK(event.signal(0u, ZX_EVENT_SIGNALED));
  const zx::time_boot after = zx::clock::get_boot();

  packet = {};
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));

  EXPECT_EQ(packet.key, 2u);
  EXPECT_EQ(packet.type, ZX_PKT_TYPE_SIGNAL_ONE);
  EXPECT_EQ(packet.signal.trigger, ZX_EVENT_SIGNALED);
  EXPECT_LE(before.get(), packet.signal.timestamp);
  EXPECT_GE(after.get(), packet.signal.timestamp);
}

TEST(PortStressTest, QueuePacketAfterPortClosedConcurrentRace) {
  constexpr size_t kIterations = 500;
  constexpr size_t kNumEvents = 32;

  for (size_t iter = 0; iter < kIterations; ++iter) {
    zx::port port;
    ASSERT_OK(zx::port::create(0u, &port));

    zx::event events[kNumEvents];
    for (size_t i = 0; i < kNumEvents; ++i) {
      ASSERT_OK(zx::event::create(0u, &events[i]));
      ASSERT_OK(events[i].wait_async(port, i + 1, ZX_EVENT_SIGNALED, 0u));
    }

    std::atomic<bool> running{true};
    std::barrier sync_point(2);
    std::thread signaler([&]() {
      sync_point.arrive_and_wait();
      while (running.load(std::memory_order_seq_cst)) {
        for (size_t i = 0; i < kNumEvents; ++i) {
          events[i].signal(0u, ZX_EVENT_SIGNALED);
          events[i].signal(ZX_EVENT_SIGNALED, 0u);
        }
      }
    });

    sync_point.arrive_and_wait();
    // Resetting the port sets zero_handles_ = true and unlinks observers while dropping
    // the lock in CallUnlocked. Concurrent signal matches encounter zero_handles_ == true,
    // safely returning ZX_ERR_BAD_HANDLE without crashing or leaking memory.
    port.reset();
    running.store(false, std::memory_order_seq_cst);
    signaler.join();
  }
}

// Tests that exceeding the maximum allocated packet limit (kMaxAllocatedPacketCountPerPort = 4096)
// triggers a policy exception with code ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_PACKETS.
TEST(PortTest, QueuePacketLimitExceededGeneratesPolicyException) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  constexpr size_t kLimit = 4096u;
  // Pre-queue 4096 ephemeral user packets.
  zx_port_packet_t packet = {.type = ZX_PKT_TYPE_USER};
  for (size_t i = 0; i < kLimit; ++i) {
    packet.key = i;
    ASSERT_OK(port.queue(&packet));
  }

  zx::event start_event;
  ASSERT_OK(zx::event::create(0u, &start_event));
  std::latch should_start(1);

  std::thread worker([&]() {
    // Wait until the main thread binds the process exception channel.
    should_start.wait();

    // Queue packets until the limit (kMaxAllocatedPacketCountPerPort = 4096) is exceeded.
    // The kernel raises ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_PACKETS and returns ZX_ERR_SHOULD_WAIT.
    while (port.queue(&packet) == ZX_OK) {
      ++packet.key;
    }
  });

  // Bind the process exception channel now that the worker thread has already started.
  zx::channel exception_channel;
  ASSERT_OK(zx_task_create_exception_channel(zx_process_self(), 0,
                                             exception_channel.reset_and_get_address()));

  // Signal the worker to attempt queueing the 4097th packet.
  should_start.count_down();

  // Wait for the policy exception on the process exception channel.
  zx_exception_info_t info = {};
  zx::exception exception;
  ASSERT_OK(exception_channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
  ASSERT_OK(exception_channel.read(0, &info, exception.reset_and_get_address(), sizeof(info), 1,
                                   nullptr, nullptr));
  EXPECT_EQ(info.type, ZX_EXCP_POLICY_ERROR);

  zx_exception_report_t report = {};
  zx::thread thread;
  ASSERT_OK(exception.get_thread(&thread));
  ASSERT_OK(
      thread.get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, &report, sizeof(report), nullptr, nullptr));
  EXPECT_EQ(report.header.type, ZX_EXCP_POLICY_ERROR);
  EXPECT_EQ(report.context.synth_code, ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_PACKETS);

  // Resume the thread from the exception and close the exception channel so
  // thread exit notifications do not block the worker thread from completing.
  uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(exception.set_property(ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)));
  exception.reset();
  exception_channel.reset();

  worker.join();
}

// Tests the race where zx_port_cancel_key cancels active PortObservers while the
// watched objects concurrently assert signals, triggering the is_canceled()
// paths in QueuePacketLocked and MaybeReapLocked across multiple observers sharing the same key.
TEST(PortStressTest, CancelKeyActiveObserverRace) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  constexpr size_t kNumEvents = 8;
  zx::event events[kNumEvents];
  for (auto& event : events) {
    ASSERT_OK(zx::event::create(0u, &event));
  }

  constexpr uint64_t kKey = 0xca4c;
  constexpr size_t kIterations = 2000;

  std::atomic<bool> done{false};

  std::thread signaler([&]() {
    while (!done.load(std::memory_order_seq_cst)) {
      for (auto& event : events) {
        event.signal(0u, ZX_USER_SIGNAL_0);
        event.signal(ZX_USER_SIGNAL_0, 0u);
      }
    }
  });

  for (size_t i = 0; i < kIterations; ++i) {
    for (auto& event : events) {
      event.wait_async(port, kKey, ZX_USER_SIGNAL_0, 0u);
    }

    zx_status_t cancel_status = port.cancel_key(0u, kKey);
    EXPECT_TRUE(cancel_status == ZX_OK || cancel_status == ZX_ERR_NOT_FOUND);

    // Drain any delivered packets to avoid queue buildup.
    zx_port_packet_t packet = {};
    while (port.wait(zx::time(0), &packet) == ZX_OK) {
    }
  }

  done.store(true, std::memory_order_seq_cst);
  signaler.join();
}

// What matters here is not so much the return values, but that the system doesn't
// crash as a result of the order. Refer to the diagram at the top of port_dispatcher.h.
TEST(PortTest, AsyncWaitCloseOrder) {
  constexpr uint64_t kEventKey = 1122;

  enum Handle { ChannelB, ChannelA, Port };
  struct CloseOrder {
    Handle first;
    Handle second;
    Handle third;
    const std::string close_list;
  };

  CloseOrder close_order_list[] = {{ChannelB, ChannelA, Port, "ChannelB, ChannelA, Port"},
                                   {ChannelB, Port, ChannelA, "ChannelB, Port, ChannelA"},
                                   {ChannelA, Port, ChannelB, "ChannelA, Port, ChannelB"},
                                   {ChannelA, ChannelB, Port, "ChannelA, ChannelB, Port"},
                                   {Port, ChannelA, ChannelB, "Port, ChannelA, ChannelB"},
                                   {Port, ChannelB, ChannelA, "Port, ChannelB, ChannelA"}};

  for (auto& a : close_order_list) {
    zx_handle_t handle[3];
    EXPECT_OK(zx_port_create(0, &handle[Port]), "%s", a.close_list.c_str());

    EXPECT_OK(zx_channel_create(0u, &handle[ChannelA], &handle[ChannelB]), "%s",
              a.close_list.c_str());

    EXPECT_OK(zx_object_wait_async(handle[ChannelB], handle[Port], kEventKey,
                                   ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, 0),
              "%s", a.close_list.c_str());

    EXPECT_OK(zx_handle_close(handle[a.first]), "%s", a.close_list.c_str());

    EXPECT_OK(zx_handle_close(handle[a.second]), "%s", a.close_list.c_str());

    EXPECT_OK(zx_handle_close(handle[a.third]), "%s", a.close_list.c_str());
  }
}

TEST(PortTest, EventAsyncSignalWaitSingle) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  constexpr uint32_t kNumAwaits = 7;

  for (uint32_t ix = 0; ix != kNumAwaits; ++ix) {
    ASSERT_OK(event.wait_async(port, ix, ZX_EVENT_SIGNALED, 0));
  }

  EXPECT_OK(event.signal(0u, ZX_EVENT_SIGNALED));

  zx_port_packet_t out = {};
  uint64_t key_sum = 0;

  for (uint32_t ix = 0; ix != (kNumAwaits - 2); ++ix) {
    EXPECT_OK(port.wait(zx::time::infinite(), &out));
    key_sum += out.key;
    EXPECT_EQ(out.type, ZX_PKT_TYPE_SIGNAL_ONE);
    EXPECT_EQ(out.signal.count, 1u);
  }

  EXPECT_EQ(key_sum, 20u);
}

TEST(PortTest, AsyncWaitEventRepeat) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  constexpr uint64_t kEventKey = 1122;

  zx_port_packet_t packet = {};
  uint64_t count[3] = {};

  constexpr uint64_t kWaitAsyncRepeats = 24;

  for (int ix = 0; ix != kWaitAsyncRepeats; ++ix) {
    ASSERT_OK(event.wait_async(port, kEventKey, ZX_EVENT_SIGNALED | ZX_USER_SIGNAL_2, 0));

    uint32_t ub = (ix % 2) ? 0u : ZX_USER_SIGNAL_2;
    // Set, then clear the signal.
    EXPECT_OK(event.signal(0u, ZX_EVENT_SIGNALED | ub));
    EXPECT_OK(event.signal(ZX_EVENT_SIGNALED | ub, 0u));

    ASSERT_OK(port.wait(zx::time::infinite_past(), &packet));
    ASSERT_EQ(packet.type, ZX_PKT_TYPE_SIGNAL_ONE);
    ASSERT_EQ(packet.signal.count, 1u);
    count[0] += (packet.signal.observed & ZX_EVENT_SIGNALED) ? 1 : 0;
    count[1] += (packet.signal.observed & ZX_USER_SIGNAL_2) ? 1 : 0;
    count[2] += (packet.signal.observed & ~(ZX_EVENT_SIGNALED | ZX_USER_SIGNAL_2)) ? 1 : 0;
  }

  EXPECT_EQ(count[0], kWaitAsyncRepeats);
  EXPECT_EQ(count[1], kWaitAsyncRepeats / 2);
  EXPECT_EQ(count[2], 0u);
}

TEST(PortTest, AsyncWaitEventManyAllProcessed) {
  constexpr uint64_t key = 6567;
  // One more than the size of the packet arena.
  constexpr size_t kEventCount = 16 * 1024 + 1;

  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx::event event[kEventCount];
  for (size_t i = 0; i < kEventCount; i++) {
    EXPECT_OK(zx::event::create(0u, &event[i]));

    EXPECT_OK(event[i].wait_async(port, key, ZX_EVENT_SIGNALED, 0));

    EXPECT_OK(event[i].signal(0u, ZX_EVENT_SIGNALED));
  }

  size_t count = 0;
  zx_port_packet_t packet = {};
  zx_status_t status;
  while (ZX_OK == (status = port.wait(zx::time::infinite_past(), &packet))) {
    EXPECT_EQ(packet.key, key);
    EXPECT_EQ(packet.type, ZX_PKT_TYPE_SIGNAL_ONE);
    EXPECT_EQ(packet.signal.observed, ZX_EVENT_SIGNALED);
    EXPECT_EQ(packet.signal.trigger, ZX_EVENT_SIGNALED);
    EXPECT_EQ(packet.signal.count, 1u);

    ++count;
  }
  EXPECT_EQ(status, ZX_ERR_TIMED_OUT);
  EXPECT_EQ(count, kEventCount);
}

// Check that zx_object_wait_async() returns an error if it is passed an
// invalid option.
TEST(PortTest, AsyncWaitInvalidOption) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  constexpr uint64_t kKey = 0;
  constexpr uint32_t kInvalidOption = 20;
  EXPECT_EQ(event.wait_async(port, kKey, ZX_EVENT_SIGNALED, kInvalidOption), ZX_ERR_INVALID_ARGS);
}

TEST(PortTest, ChannelAsyncWaitOnExistingStateIsNotified) {
  constexpr uint64_t kEventKey = 65667;

  // Create a channel pair, and write 5 messages into it.
  zx::channel ch[2];
  ASSERT_OK(zx::channel::create(0u, &ch[0], &ch[1]));
  for (int ix = 0; ix != 5; ++ix) {
    ASSERT_OK(ch[0].write(0u, "123456", 6, nullptr, 0u));
  }
  ch[0].reset();

  // Create a port, and set it up to be notified when the channel is
  // readable or closed.
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(ch[1].wait_async(port, kEventKey, ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, 0));

  // Wait for a packet to be received on the port, with both the
  // READABLE and PEER_CLOSED signals asserted.
  zx_port_packet_t packet = {};
  zx_status_t status = port.wait(zx::time::infinite_past(), &packet);
  EXPECT_EQ(status, ZX_OK);
  EXPECT_EQ(packet.signal.count, 1u);  // count is always 1.
  EXPECT_EQ(packet.signal.trigger, ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED);
  EXPECT_EQ(packet.signal.observed, ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED);

  // We don't expect any other events on the port.
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &packet), ZX_ERR_TIMED_OUT);
}

TEST(PortTest, CancelEventKey) {
  zx::port port;
  zx::event event;

  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  // Notice repeated key below.
  const uint64_t keys[] = {128u, 13u, 7u, 13u};

  for (uint32_t ix = 0; ix != std::size(keys); ++ix) {
    ASSERT_OK(event.wait_async(port, keys[ix], ZX_EVENT_SIGNALED, 0));
  }

  // We cancel before it is signaled so no packets from |13| are seen.
  EXPECT_OK(port.cancel(event, 13u));

  for (int ix = 0; ix != 2; ++ix) {
    EXPECT_OK(event.signal(0u, ZX_EVENT_SIGNALED));
    EXPECT_OK(event.signal(ZX_EVENT_SIGNALED, 0u));
  }

  zx_port_packet_t packet = {};
  int wait_count = 0;
  uint64_t key_sum = 0;

  zx_status_t status;
  while (true) {
    status = port.wait(zx::time::infinite_past(), &packet);
    if (status != ZX_OK) {
      break;
    }
    wait_count++;
    key_sum += packet.key;
    EXPECT_EQ(packet.signal.trigger, ZX_EVENT_SIGNALED);
    EXPECT_EQ(packet.signal.observed, ZX_EVENT_SIGNALED);
  }

  // We cancel after the packet has been delivered.
  EXPECT_EQ(port.cancel(event, 128u), ZX_ERR_NOT_FOUND);

  EXPECT_EQ(wait_count, 2);
  EXPECT_EQ(key_sum, keys[0] + keys[2]);
}

TEST(PortTest, CancelEventKeyAfter) {
  zx::port port;

  ASSERT_OK(zx::port::create(0u, &port));

  const uint64_t keys[] = {128u, 3u, 3u};
  zx::event ev[std::size(keys)];
  for (uint32_t ix = 0; ix != std::size(keys); ++ix) {
    ASSERT_OK(zx::event::create(0u, &ev[ix]));
    ASSERT_OK(ev[ix].wait_async(port, keys[ix], ZX_EVENT_SIGNALED, 0));
  }

  EXPECT_OK(ev[0].signal(0u, ZX_EVENT_SIGNALED));
  EXPECT_OK(ev[1].signal(0u, ZX_EVENT_SIGNALED));

  // We cancel after the first two signals and before the third. So it should
  // test both cases with queued packets and no-yet-fired packets.
  EXPECT_OK(port.cancel(ev[1], 3u));
  EXPECT_OK(port.cancel(ev[2], 3u));

  EXPECT_OK(ev[2].signal(0u, ZX_EVENT_SIGNALED));

  zx_port_packet_t packet = {};
  int wait_count = 0;
  uint64_t key_sum = 0;

  zx_status_t status;
  while (true) {
    status = port.wait(zx::time::infinite_past(), &packet);
    if (status != ZX_OK) {
      break;
    }
    wait_count++;
    key_sum += packet.key;
    EXPECT_EQ(packet.signal.trigger, ZX_EVENT_SIGNALED);
    EXPECT_EQ(packet.signal.observed, ZX_EVENT_SIGNALED);
  }

  EXPECT_EQ(wait_count, 1);
  EXPECT_EQ(key_sum, keys[0]);
}

TEST(PortTest, CancelKeyInvalidOptions) {
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  // Currently, no options are supported.
  EXPECT_EQ(port.cancel_key(1, 0), ZX_ERR_INVALID_ARGS);
}

TEST(PortTest, CancelKey) {
  zx::event event;
  zx::port port;

  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(zx::event::create(0, &event));

  ASSERT_OK(event.wait_async(port, 1, ZX_EVENT_SIGNALED, 0));
  ASSERT_OK(event.wait_async(port, 2, ZX_EVENT_SIGNALED, 0));
  ASSERT_OK(event.wait_async(port, 2, ZX_USER_SIGNAL_0, 0));

  EXPECT_EQ(port.cancel_key(0, 1), ZX_OK);

  ASSERT_OK(event.signal(0u, ZX_EVENT_SIGNALED));

  // This should remove the packet for the ZX_EVENT_SIGNALED wait and cancel the pending
  // wait for ZX_USER_SIGNAL_0.
  EXPECT_EQ(port.cancel_key(0, 2), ZX_OK);
  // Now that the wait is canceled subsequent calls should return NOT_FOUND.
  EXPECT_EQ(port.cancel_key(0, 2), ZX_ERR_NOT_FOUND);

  zx_port_packet_t packet = {};
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &packet), ZX_ERR_TIMED_OUT);

  ASSERT_OK(event.signal(0u, ZX_USER_SIGNAL_0));
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &packet), ZX_ERR_TIMED_OUT);

  EXPECT_EQ(port.cancel_key(0, 3), ZX_ERR_NOT_FOUND);
}

TEST(PortTest, CancelKeyQueuedUserPacket) {
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  constexpr uint64_t kKey = 42u;
  zx_port_packet_t packet = {
      .key = kKey,
      .type = ZX_PKT_TYPE_USER,
      .status = ZX_OK,
  };
  ASSERT_OK(port.queue(&packet));

  // Cancel the queued user packet by key, which removes and frees the ephemeral packet.
  EXPECT_EQ(port.cancel_key(0u, kKey), ZX_OK);

  // Subsequent cancellation of the same key should return ZX_ERR_NOT_FOUND.
  EXPECT_EQ(port.cancel_key(0u, kKey), ZX_ERR_NOT_FOUND);

  // Verify that the port queue is empty and the canceled packet cannot be dequeued.
  zx_port_packet_t out = {};
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &out), ZX_ERR_TIMED_OUT);
}

// Set up a scenario where one thread dequeues a packet while another thread raises an observed
// signal on an object and another thread attempts to cancel the wait via zx_port_cancel_key().
// Verified that either zx_port_wait() returns a packet or zx_port_cancel_key() returns ZX_OK, but
// not both for any wait.
TEST(PortStressTest, CancelKeyDuringMatchRace) {
  zx::port port;

  zx::event race_event, cancel_event;
  ASSERT_OK(zx::event::create(0u, &race_event));
  ASSERT_OK(zx::event::create(0u, &cancel_event));

  constexpr uint64_t kCancelEventKey = 1;

  uint64_t race_event_packets{
      0};  // Stored only by wait_thread, read only after this thread is joined.
  uint64_t race_event_cancel_not_found{0};
  uint64_t race_event_canceled{0};

  // Flow:
  // - All threads prepare and wait on the ready barrier.
  // - wait_thread waits on the port.
  // - cancel_thread cancels the wait.
  // - signal_thread signals the event, waits a bit, then signals the cancel event.
  // - wait_thread and cancel_thread report results into counters/log.
  // - All threads wait on the complete barrier.

  uint64_t race_key = 0xf0f0f0f0;
  std::barrier ready(3);
  std::barrier complete(3);

  constexpr size_t kIters = 500;

  // Thread 'wait_thread' will be dequeuing packets using zx_port_wait() and
  // counting how many packets it receives for the racing object.
  auto wait_thread = std::thread([&]() {
    for (size_t i = 0; i < kIters; ++i) {
      // Create a fresh port for each iteration.
      ASSERT_OK(zx::port::create(0u, &port));

      // Clear signals on events.
      race_event.signal(ZX_EVENT_SIGNALED, 0u);
      cancel_event.signal(ZX_EVENT_SIGNALED, 0u);

      // Wait on the port until either the race or cancel event has a signal raised.
      race_event.wait_async(port, race_key, ZX_EVENT_SIGNALED, 0);
      cancel_event.wait_async(port, kCancelEventKey, ZX_EVENT_SIGNALED, 0);

      ready.arrive_and_wait();
      zx_port_packet_t packet{};
      zx_status_t wait_status = port.wait(zx::time::infinite(), &packet);
      ZX_ASSERT_MSG(wait_status == ZX_OK, "wait_status: %u", wait_status);
      if (packet.key == race_key) {
        ++race_event_packets;
      } else {
        ZX_ASSERT_MSG(packet.key == kCancelEventKey, "key: %lu", packet.key);
      }

      // Let other threads know we're done.
      complete.arrive_and_wait();
    }
  });

  auto signal_thread = std::thread([&]() {
    for (size_t i = 0; i < kIters; ++i) {
      ready.arrive_and_wait();
      race_event.signal(0u, ZX_EVENT_SIGNALED);
      // Sleep for a little bit to give the signal on race_event a chance to trigger.
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
      // Signal cancel_event to get wait_thread out of the port wait even if the wait on
      // race_event is successfully canceled.
      cancel_event.signal(0u, ZX_EVENT_SIGNALED);
      complete.arrive_and_wait();
    }
  });

  auto cancel_thread = std::thread([&]() {
    for (size_t i = 0; i < kIters; ++i) {
      ready.arrive_and_wait();
      zx_status_t status = port.cancel_key(0u, race_key);
      if (status == ZX_OK) {
        ++race_event_canceled;
      } else if (status == ZX_ERR_NOT_FOUND) {
        ++race_event_cancel_not_found;
      } else {
        ZX_ASSERT_MSG(false, "unexpected status from zx_port_cancel_key: %u", status);
      }
      complete.arrive_and_wait();
    }
  });

  wait_thread.join();
  signal_thread.join();
  cancel_thread.join();

  // For each iteration, we should have either generated a port packet or a canceled status.
  EXPECT_EQ(race_event_packets, race_event_cancel_not_found);
  EXPECT_EQ(race_event_canceled + race_event_cancel_not_found, kIters);
}

// Test racing zx_port_cancel_key with zx_object_signal when multiple objects share the same key.
TEST(PortStressTest, CancelKeySharedKeyRace) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  constexpr size_t kNumEvents = 1000;
  std::vector<zx::event> events(kNumEvents);
  const uint64_t key = 0x1234;

  for (size_t i = 0; i < kNumEvents; i++) {
    ASSERT_OK(zx::event::create(0u, &events[i]));
    ASSERT_OK(events[i].wait_async(port, key, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_ONCE));
  }

  std::atomic<bool> proceed{false};
  auto signal_thread = std::thread([&]() {
    while (!proceed.load()) {
      std::this_thread::yield();
    }
    for (size_t i = 0; i < kNumEvents; i++) {
      events[i].signal(0u, ZX_EVENT_SIGNALED);
    }
  });

  proceed.store(true);

  // This should trigger the race
  zx_status_t status = port.cancel_key(0u, key);

  // Both OK and NOT_FOUND are acceptable depending on timing,
  // but it should not crash or hang.
  EXPECT_TRUE(status == ZX_OK || status == ZX_ERR_NOT_FOUND);

  signal_thread.join();
}

// Tests matching a port observer concurrently with closing the last handle to the port.
TEST(PortStressTest, MatchHandleCloseRace) {
  zx::port port;

  zx::event race_event;
  ASSERT_OK(zx::event::create(0u, &race_event));

  // Flow:
  // - Both threads wait for the ready barrier.
  // - Match thread raises signal on waited-on event.
  // - This thread closes handle to port.
  // - Both threads wait for the complete barrier.

  std::barrier ready(2);
  std::barrier complete(2);

  constexpr size_t kIters = 20000;

  auto match_thread = std::thread([&]() {
    for (size_t i = 0; i < kIters; ++i) {
      ready.arrive_and_wait();

      ASSERT_OK(race_event.signal(0u, ZX_EVENT_SIGNALED));

      complete.arrive_and_wait();
    }
  });

  for (size_t i = 0; i < kIters; ++i) {
    ASSERT_OK(zx::port::create(0u, &port));
    ASSERT_OK(race_event.wait_async(port, 1, ZX_EVENT_SIGNALED, 0));
    ready.arrive_and_wait();
    port.reset();
    complete.arrive_and_wait();
  }

  match_thread.join();
}

// This test validates that a timeout in a channel call does not trigger a panic.
// This is a regression test for https://fxbug.dev/512083099
TEST(PortStressTest, ChannelCallWaitQueuePanicRepro) {
  zx::channel ch2_req, ch2_rep;
  ASSERT_OK(zx::channel::create(0, &ch2_req, &ch2_rep));

  std::atomic<bool> keep_running = true;
  std::atomic<uint32_t> message_count = 0;

  // Thread C (Event thread)
  std::thread t_c([&]() {
    char name[] = "channel-call-thread-c";
    zx::thread::self()->set_property(ZX_PROP_NAME, name, sizeof(name));

    zx_signals_t observed;
    char buf[8] = {0};
    uint32_t act_bytes, act_handles;
    while (keep_running) {
      ch2_rep.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &observed);
      ch2_rep.read(0, buf, nullptr, 8, 0, &act_bytes, &act_handles);
      message_count.fetch_sub(1);
    }
  });

  for (size_t i = 0; i < 1000; ++i) {
    zx::channel ch1_req, ch1_rep;
    ASSERT_OK(zx::channel::create(0, &ch1_req, &ch1_rep));

    zx::port port;
    ASSERT_OK(zx::port::create(0, &port));

    ASSERT_OK(ch1_rep.wait_async(port, 123, ZX_CHANNEL_READABLE, 0));

    std::atomic<bool> thread_a_timed_out = false;

    // Thread B
    std::thread t_b([&]() {
      char name[] = "channel-call-thread-b";
      zx::thread::self()->set_property(ZX_PROP_NAME, name, sizeof(name));

      zx_port_packet_t packet;
      port.wait(zx::time::infinite(), &packet);

      while (!thread_a_timed_out) {
        std::this_thread::yield();
      }
    });

    // Thread A
    std::thread t_a([&]() {
      char name[] = "channel-call-thread-a";
      zx::thread::self()->set_property(ZX_PROP_NAME, name, sizeof(name));

      char buf[8] = {0};
      zx_channel_call_args_t args = {
          .wr_bytes = buf,
          .wr_handles = nullptr,
          .rd_bytes = buf,
          .rd_handles = nullptr,
          .wr_num_bytes = 8,
          .wr_num_handles = 0,
          .rd_num_bytes = 8,
          .rd_num_handles = 0,
      };
      uint32_t act_bytes, act_handles;

      // Call 1: make Thread B the owner
      ch1_req.call(0, zx::deadline_after(zx::usec(1)), &args, &act_bytes, &act_handles);
      thread_a_timed_out = true;

      zx::nanosleep(zx::deadline_after(zx::usec(10)));

      // Call 2: trigger Event::Signal on Thread C
      for (int i = 0; i < 50; i++) {
        ch2_req.call(0, zx::deadline_after(zx::usec(10)), &args, &act_bytes, &act_handles);
        // Wait for C to read and block again
        zx::nanosleep(zx::deadline_after(zx::usec(10)));
        // If the outstanding message count is getting high, wait for C to catch up.
        if (uint32_t count = message_count.fetch_add(1); count > 2000) {
          while (message_count.load() > 1000) {
            std::this_thread::yield();
          }
        }
      }
    });

    t_a.join();
    t_b.join();
  }

  keep_running = false;
  // Wake up C to exit
  char buf[8] = {0};
  ch2_req.write(0, buf, 8, nullptr, 0);
  t_c.join();
}

TEST(PortTest, ThreadEvents) {
  zx::port port;
  zx::event event;
  constexpr size_t kNumPortWaiterThreads = 3;

  auto PortWaiter = [](zx::port* port, uint32_t count, zx_status_t* return_status) {
    zx_port_packet_t packet = {};
    do {
      *return_status = port->wait(zx::time::infinite(), &packet);
      if (*return_status < 0) {
        return;
      }
    } while (--count);
  };

  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  std::thread port_waiters[kNumPortWaiterThreads];
  zx_status_t return_status[kNumPortWaiterThreads];
  std::fill_n(return_status, kNumPortWaiterThreads, ZX_ERR_INTERNAL);

  for (size_t ix = 0; ix != kNumPortWaiterThreads; ++ix) {
    // |count| is one so each thread is going to pick one packet each
    // and exit. See bug https://fxbug.dev/42105487 for the case this is testing.
    EXPECT_OK(event.wait_async(port, (500u + ix), ZX_EVENT_SIGNALED, 0));

    port_waiters[ix] = std::thread(PortWaiter, &port, 1, &return_status[ix]);
  }

  EXPECT_OK(event.signal(0u, ZX_EVENT_SIGNALED));

  for (size_t ix = 0; ix != kNumPortWaiterThreads; ++ix) {
    port_waiters[ix].join();
    EXPECT_OK(return_status[ix]);
  }
}

TEST(PortTest, Timestamp) {
  // Test that the timestamp feature returns reasonable numbers. We use
  // a single thread so the numbers should be nanosecond-grade reliable.
  zx::port port;
  zx::event event[2];
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event[0]));
  ASSERT_OK(zx::event::create(0u, &event[1]));

  ASSERT_OK(event[0].wait_async(port, 1, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_TIMESTAMP));
  ASSERT_OK(event[1].wait_async(port, 2, ZX_EVENT_SIGNALED, 0u));

  auto before = zx::clock::get_monotonic();
  ASSERT_OK(event[0].signal(0u, ZX_EVENT_SIGNALED));
  auto after = zx::clock::get_monotonic();
  ASSERT_OK(event[1].signal(0u, ZX_EVENT_SIGNALED));

  zx_port_packet_t packet[2];
  ASSERT_OK(port.wait(zx::time::infinite(), &packet[0]));
  ASSERT_OK(port.wait(zx::time::infinite(), &packet[1]));

  ASSERT_EQ(packet[0].signal.trigger, ZX_EVENT_SIGNALED);
  ASSERT_EQ(packet[1].signal.trigger, ZX_EVENT_SIGNALED);

  EXPECT_LE(before, zx::time(packet[0].signal.timestamp));
  EXPECT_GE(after, zx::time(packet[0].signal.timestamp));

  EXPECT_EQ(0u, packet[1].signal.timestamp);

  // Now test the same using event[0] in place of event[1].
  ASSERT_OK(event[0].signal(ZX_EVENT_SIGNALED, 0u));
  ASSERT_OK(event[1].signal(ZX_EVENT_SIGNALED, 0u));

  ASSERT_OK(event[1].wait_async(port, 1, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_TIMESTAMP));
  ASSERT_OK(event[0].wait_async(port, 2, ZX_EVENT_SIGNALED, 0u));

  ASSERT_OK(event[0].signal(0u, ZX_EVENT_SIGNALED));
  before = zx::clock::get_monotonic();
  ASSERT_OK(event[1].signal(0u, ZX_EVENT_SIGNALED));
  after = zx::clock::get_monotonic();

  ASSERT_OK(port.wait(zx::time::infinite(), &packet[0]));
  ASSERT_OK(port.wait(zx::time::infinite(), &packet[1]));

  EXPECT_LE(before, zx::time(packet[1].signal.timestamp));
  EXPECT_GE(after, zx::time(packet[1].signal.timestamp));

  EXPECT_EQ(0u, packet[0].signal.timestamp);
}

TEST(PortTest, EdgeTriggerDetected) {
  // Test ZX_WAIT_ASYNC_EDGE only generates a packet when a signal
  // becomes active after the wait_async.
  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  uint64_t key = 2u;
  ASSERT_OK(event.wait_async(port, key, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_EDGE));

  zx_handle_t main_thread = zx_thread_self();

  auto loop_signal = [&event, main_thread]() {
    zx_info_thread thd_info;
    do {
      // This sleep is to prevent repeated calls to zx_object_get_info.
      // The test is deterministic.
      zx_nanosleep(zx_deadline_after(ZX_USEC(600)));
      EXPECT_OK(zx_object_get_info(main_thread, ZX_INFO_THREAD, &thd_info, sizeof(thd_info),
                                   nullptr, nullptr));
    } while (thd_info.state != ZX_THREAD_STATE_BLOCKED_PORT);
    EXPECT_OK(event.signal(0u, ZX_EVENT_SIGNALED));
  };
  std::thread signal_thread(loop_signal);
  zx_port_packet_t packet;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  ASSERT_OK(packet.status);
  ASSERT_EQ(ZX_PKT_TYPE_SIGNAL_ONE, packet.type);
  ASSERT_EQ(ZX_EVENT_SIGNALED, packet.signal.observed);
  ASSERT_EQ(key, packet.key);

  signal_thread.join();
}

TEST(PortTest, EdgeTriggerCancel) {
  // This is the same as EdgeTriggerDetected, but the wait
  // is cancelled before the signal is set.
  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  uint64_t key1 = 42u;
  uint64_t key2 = 999u;
  uint64_t key3 = 2020u;

  ASSERT_OK(event.wait_async(port, key1, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_EDGE));
  ASSERT_OK(event.wait_async(port, key2, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_EDGE));
  ASSERT_OK(event.wait_async(port, key3, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_EDGE));

  EXPECT_OK(port.cancel(event, key2));
  ASSERT_OK(event.signal(0u, ZX_EVENT_SIGNALED));
  EXPECT_OK(port.cancel(event, key3));

  zx_port_packet_t packet;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  ASSERT_OK(packet.status);
  ASSERT_EQ(ZX_PKT_TYPE_SIGNAL_ONE, packet.type);
  ASSERT_EQ(ZX_EVENT_SIGNALED, packet.signal.observed);
  ASSERT_EQ(key1, packet.key);

  // We cancelled key2 and key3, so they should not be queued.
  ASSERT_EQ(ZX_ERR_TIMED_OUT, port.wait(zx::time(zx_deadline_after(ZX_USEC(600))), &packet));
}

TEST(PortTest, EdgeTriggerTimeout) {
  // Test ZX_WAIT_ASYNC_EDGE only generates a packet when a signal
  // becomes active after the wait_async.
  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  // Call signal before wait, so no packet will be queued.
  ASSERT_OK(event.signal(0u, ZX_EVENT_SIGNALED));
  ASSERT_OK(event.wait_async(port, 1, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_EDGE));

  zx_port_packet_t packet;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, port.wait(zx::time(zx_deadline_after(ZX_USEC(600))), &packet));
}

// Queue a packet while another thread is closing the port.  See that queuing thread observes
// ZX_ERR_BAD_HANDLE. This test is inherently racy in that we can not always get both the
// closing thread and the queuing thread to enter the code under test at the same time.
TEST(PortTest, CloseQueueRace) {
  zx::port port;
  std::atomic<uint64_t> count = 0;
  ASSERT_OK(zx::port::create(0, &port));

  auto queue_loop = [port = port.borrow(), &count]() {
    constexpr zx_port_packet_t kPacket = {1ull, ZX_PKT_TYPE_USER, 0, {{}}};
    constexpr uint64_t kMaxMessages = 1024;
    uint64_t enqueued_messages = 0;

    while (true) {
      // Enqueue a message.
      {
        zx_status_t status = port->queue(&kPacket);
        if (status != ZX_OK) {
          ZX_ASSERT_MSG(status == ZX_ERR_BAD_HANDLE, "Unexpected status: %d", status);
          break;
        }
        enqueued_messages++;
      }

      // If too many messages are enqueued, dequeue one to avoid hitting the
      // port depth limit.
      while (enqueued_messages >= kMaxMessages) {
        zx_port_packet read_packet;
        zx_status_t status = port->wait(zx::time::infinite_past(), &read_packet);
        if (status == ZX_ERR_TIMED_OUT) {
          // We may race with the port being closed in the following manner:
          //
          //  1. This thread enters the kernel, converts its handle into
          //     a reference to the port object, but doesn't yet read
          //     the messages on the queue.
          //
          //  2. The other thread deletes the handle to the port,
          //     deleting all pending messages in the process.
          //
          //  3. Because no message is waiting, the port operation times
          //     out with ZX_ERR_TIMED_OUT.
          //
          // In this case, we just try again.
          continue;
        } else if (status == ZX_ERR_BAD_HANDLE) {
          // Port was closed.
          break;
        }
        ZX_ASSERT_MSG(status == ZX_OK, "Unexpected status: %d", status);
        enqueued_messages--;
      }

      count.fetch_add(1);
    }
  };

  std::thread queue_thread(queue_loop);

  // Spin until |queue_thread| completes at least one loop iteration.
  while (count.load() == 0) {
  }

  // Close the port out from under it.
  port.reset();

  // See that it gets ZX_ERR_BAD_HANDLE.
  queue_thread.join();
}

// See that creating too many object observers raises an exception.
TEST(PortTest, TooManyObservers) {
  if (getenv("NO_NEW_PROCESS")) {
    ZXTEST_SKIP("Running without the ZX_POL_NEW_PROCESS policy, skipping test case.");
  }

  constexpr char kName[] = "too-many-observers-test";
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;
  ASSERT_OK(
      zx::process::create(*zx::job::default_job(), kName, sizeof(kName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kName, sizeof(kName) - 1, 0u, &thread));

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::channel cmd_channel;
  ASSERT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), local.release(), true,
                                   cmd_channel.reset_and_get_address()));
  // If the process is terminated as expected, we should peer closed.
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, mini_process_cmd(cmd_channel.get(), MINIP_CMD_WAIT_ASYNC, nullptr));
  ASSERT_OK(proc.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), nullptr));

  zx_info_process_t proc_info;
  ASSERT_OK(proc.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr));
  ASSERT_EQ(proc_info.return_code, ZX_TASK_RETCODE_EXCEPTION_KILL);
}

TEST(PortStressTest, WaitSignalCancel) {
  // This tests a race that existed between the port observer
  // removing itself from the event and the cancellation logic which is
  // also working with the same internal object. The net effect of the
  // bug is that port_cancel() would fail with ZX_ERR_NOT_FOUND.
  //
  // When running on real hardware or KVM-accelerated emulation
  // a good number to set for kStressCount is 50000000.
  constexpr uint32_t kStressCount = 20000;

  auto CancelStressWaiter = [](zx::port* port, zx::event* event, zx_status_t* return_status) {
    const auto key = 919u;
    auto count = kStressCount;

    while (--count) {
      *return_status = event->wait_async(*port, key, ZX_EVENT_SIGNALED, 0);
      if (*return_status != ZX_OK) {
        break;
      }

      zx_signals_t observed;
      *return_status = event->wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), &observed);
      if (*return_status != ZX_OK) {
        break;
      }

      *return_status = port->cancel(*event, key);
      if (*return_status != ZX_OK) {
        break;
      }
    }
  };

  auto CancelStressSignaler = [](zx::port* port, zx::event* event,
                                 std::atomic<bool>* keep_running) {
    uint64_t count = 0;
    zx_status_t status;

    while (keep_running->load()) {
      status = event->signal(0u, ZX_EVENT_SIGNALED);
      if (status != ZX_OK) {
        return;
      }

      constexpr uint32_t kSleeps[] = {0, 10, 2, 0, 15, 0};
      auto duration = kSleeps[count++ % std::size(kSleeps)];
      if (duration > 0) {
        zx::nanosleep(zx::deadline_after(zx::nsec(duration)));
      }

      status = event->signal(ZX_EVENT_SIGNALED, 0u);
      if (status != ZX_OK) {
        return;
      }
    }
  };

  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));
  zx_status_t waiter_status = ZX_ERR_INTERNAL;
  std::atomic<bool> keep_running(true);

  std::thread waiter(CancelStressWaiter, &port, &event, &waiter_status);
  std::thread signaler(CancelStressSignaler, &port, &event, &keep_running);

  waiter.join();
  keep_running.store(false);
  signaler.join();

  EXPECT_OK(waiter_status);
}

// A stress test that repeatedly signals and closes events registered with a port.
TEST(PortStressTest, SignalCloseWait) {
  constexpr zx::duration kTestDuration = zx::msec(100);
  srand(4);

  // Continually reads packets from a port until it gets a ZX_PKT_TYPE_USER.
  auto PortWaitDrainer = [](zx::port* port, zx_status_t* return_status) {
    while (true) {
      zx_port_packet_t packet{};
      zx_status_t status = port->wait(zx::time::infinite(), &packet);
      if (status != ZX_OK) {
        *return_status = status;
        return;
      }
      if (packet.type == ZX_PKT_TYPE_USER) {
        *return_status = ZX_OK;
        break;
      }
    }
  };

  // Creates an event registered with the port then performs the following actions randomly:
  //   a. sleep
  //   b. signal the event
  //   c. signal the event, then close it
  auto WaitEventSignalClose = [](zx::port* port, std::atomic<bool>* keep_running,
                                 zx_status_t* return_status) {
    zx_status_t status;
    zx::event event;

    while (keep_running->load()) {
      if (!event.is_valid()) {
        status = zx::event::create(0u, &event);
        if (status != ZX_OK) {
          *return_status = status;
          return;
        }
        status = event.wait_async(*port, 0, ZX_EVENT_SIGNALED, 0);
        if (status != ZX_OK) {
          *return_status = status;
          return;
        }
      }

      unsigned action = rand() % 3;
      switch (action) {
        case 0:  // sleep
          zx::nanosleep(zx::deadline_after(zx::msec(1)));
          break;
        case 1:  // signal
          status = event.signal(0u, ZX_EVENT_SIGNALED);
          if (status != ZX_OK) {
            *return_status = status;
            return;
          }
          break;
        default:  // signal and close
          status = event.signal(0u, ZX_EVENT_SIGNALED);
          if (status != ZX_OK) {
            *return_status = status;
            return;
          }
          event.reset();
      }
    }
    *return_status = ZX_OK;
  };

  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  constexpr unsigned kNumSignalers = 4;
  std::thread signalers[kNumSignalers];
  zx_status_t signaler_return_status[kNumSignalers];
  std::fill_n(signaler_return_status, kNumSignalers, ZX_ERR_INTERNAL);
  std::atomic<bool> keep_running(true);

  for (size_t ix = 0; ix != kNumSignalers; ++ix) {
    signalers[ix] =
        std::thread(WaitEventSignalClose, &port, &keep_running, &signaler_return_status[ix]);
  }

  constexpr unsigned kNumDrainers = 4;
  std::thread drainers[kNumDrainers];
  zx_status_t drainer_return_status[kNumDrainers];
  std::fill_n(drainer_return_status, kNumSignalers, ZX_ERR_INTERNAL);
  for (size_t ix = 0; ix != kNumDrainers; ++ix) {
    drainers[ix] = std::thread(PortWaitDrainer, &port, &drainer_return_status[ix]);
  }

  zx::nanosleep(zx::deadline_after(kTestDuration));
  keep_running.store(false);

  for (size_t ix = 0; ix < kNumDrainers; ++ix) {
    zx_port_packet_t pkt{};
    pkt.type = ZX_PKT_TYPE_USER;
    zx_status_t status;
    do {
      status = port.queue(&pkt);
    } while (status == ZX_ERR_SHOULD_WAIT);
    EXPECT_OK(status);
  }

  for (size_t ix = 0; ix < kNumDrainers; ++ix) {
    drainers[ix].join();
    EXPECT_OK(drainer_return_status[ix]);
  }

  for (size_t ix = 0; ix < kNumSignalers; ++ix) {
    signalers[ix].join();
    EXPECT_OK(signaler_return_status[ix]);
  }
}

// A stress test designed to create a race where one thread is closing the port as another thread is
// performing an object_wait_async using the same port handle.
TEST(PortStressTest, CloseWaitRace) {
  constexpr zx::duration kTestDuration = zx::msec(100);
  srand(4);

  constexpr uint64_t kNumWaiters = 4;
  constexpr uint64_t kTotalThreads = kNumWaiters + 1;
  struct Args {
    std::atomic<uint64_t>* ready_count{};
    std::atomic<bool>* keep_running{};
    std::atomic<zx_handle_t>* port{};
    zx_status_t status{ZX_ERR_INTERNAL};
  };

  // Repeatedly asynchronously wait on an event.
  auto WaitAsyncLoop = [](Args* args) {
    // Keep track of how many async waits we perform so we can self-limit and stay below any kernel
    // imposed maximum.
    constexpr uint64_t kMaxWaitsPerObject = 100;
    uint64_t num_waits = 0;

    // Create an event that we'll async_wait on.
    zx::event event;
    args->status = zx::event::create(0, &event);
    if (args->status != ZX_OK) {
      return;
    }

    // Signal that we're ready.
    args->ready_count->fetch_add(1);
    // Wait for all the other threads to become ready.
    while (args->ready_count->load() < kTotalThreads) {
    }
    while (args->keep_running->load()) {
      // We're racing with another thread that is closing the port we're about to use.  There are
      // three expected cases for the wait_async call below.
      //
      // First, if we "win" the race and
      // complete our wait_async before the other thread closes the port, then we'll see ZX_OK.
      //
      // Second, if the other thread "wins" and closes the port, then we might see
      // ZX_ERR_BAD_HANDLE.
      //
      // The third case is less obvious.  Let's say the other thread closes the port just after our
      // wait_async returns ZX_OK.  However, it hasn't gotten around to replacing the port handle
      // value with ZX_HANDLE_INVALID.  We continue on, see that the num_waits value has exceeded
      // the threshold and so we create another event.  It's possible that the handle value chosen
      // for this new event is the recycle port handle value that's still sitting in args->port.  If
      // that happens, then in the subsequent iteration zx_object_wait_async will return
      // ZX_ERR_WRONG_TYPE because unlikely events, ports aren't waitable.
      zx_status_t status =
          zx_object_wait_async(event.get(), args->port->load(), 0, ZX_EVENT_SIGNALED, 0);
      if (status != ZX_OK && status != ZX_ERR_BAD_HANDLE && status != ZX_ERR_WRONG_TYPE) {
        args->status = status;
        return;
      }
      if (++num_waits >= kMaxWaitsPerObject) {
        args->status = zx::event::create(0, &event);
        if (args->status != ZX_OK) {
          return;
        }

        num_waits = 0;
      }
    }
    args->status = ZX_OK;
  };

  // Repeatedly create and close a port.
  auto CreatePortLoop = [](Args* args) {
    // Signal that we're ready.
    args->ready_count->fetch_add(1);
    // Wait for all the other threads to become ready.
    while (args->ready_count->load() < kTotalThreads) {
    }
    while (args->keep_running->load()) {
      zx_handle_t temp_port;
      zx_status_t status = zx_port_create(0, &temp_port);
      if (status != ZX_OK) {
        args->status = status;
        return;
      }
      args->port->store(temp_port);

      // Give the waiter threads an opportunity to get the handle and wait_async on it.
      zx::nanosleep(zx::deadline_after(zx::msec(1)));

      // Then close it out from under them.
      status = zx_handle_close(temp_port);
      args->port->store(ZX_HANDLE_INVALID);
      if (status != ZX_OK) {
        args->status = status;
        return;
      }
    }
    args->status = ZX_OK;
  };

  std::atomic<uint64_t> ready_count(0);
  std::atomic<bool> keep_running(true);
  std::atomic<zx_handle_t> port(ZX_HANDLE_INVALID);

  Args waiter_args[kNumWaiters];
  std::thread wait_async_thread[kNumWaiters];
  for (size_t ix = 0; ix != kNumWaiters; ++ix) {
    Args* args = &waiter_args[ix];
    args->ready_count = &ready_count;
    args->keep_running = &keep_running;
    args->port = &port;
    wait_async_thread[ix] = std::thread(WaitAsyncLoop, args);
  }

  Args creator_args{&ready_count, &keep_running, &port, ZX_ERR_INTERNAL};
  std::thread create_port_thread(CreatePortLoop, &creator_args);

  zx::nanosleep(zx::deadline_after(kTestDuration));
  keep_running.store(false);

  for (size_t ix = 0; ix != kNumWaiters; ++ix) {
    wait_async_thread[ix].join();
    ASSERT_OK(waiter_args[ix].status);
  }

  create_port_thread.join();
  EXPECT_OK(creator_args.status);

  zx_handle_close(port.load());
}

// This test registers and matches asynchronous waits on an object while simultaneously attempting
// to cancel the waits by key from another thread.
// This is a regression test for https://fxbug.dev/504783349
TEST(PortStressTest, CancelKeyDuringRegistrationRace) {
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  constexpr uint64_t kKey = 1234u;

  std::atomic<bool> run{true};

  std::thread signaling_thread([&]() {
    while (run) {
      event.signal(0, ZX_EVENT_SIGNALED);
      event.signal(ZX_EVENT_SIGNALED, 0);
    }
  });

  std::thread canceling_thread([&]() {
    while (run) {
      port.cancel_key(0, kKey);
    }
  });

  std::thread waiting_thread([&]() {
    while (run) {
      zx_port_packet_t packet;
      port.wait(zx::deadline_after(zx::msec(5)), &packet);
    }
  });

  for (int i = 0; i < 100000; i++) {
    event.wait_async(port, kKey, ZX_EVENT_SIGNALED, 0);
  }

  run.store(false);
  signaling_thread.join();
  canceling_thread.join();
  waiting_thread.join();
}

// This is another variation of CancelKeyDuringRegistrationRace that focuses on closing handles
// and canceling waits registered on the same object.
// This is a regression test for https://fxbug.dev/504783349
TEST(PortStressTest, CancelKeyCloseRace) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  const uint64_t key = 0x1234;

  constexpr size_t kNumDummies = 10000;
  std::vector<zx::event> dummy_events(kNumDummies);
  for (size_t i = 0; i < kNumDummies; i++) {
    ASSERT_OK(zx::event::create(0u, &dummy_events[i]));
    ASSERT_OK(dummy_events[i].wait_async(port, key, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_ONCE));
  }

  zx::event event_b;
  ASSERT_OK(zx::event::create(0u, &event_b));
  ASSERT_OK(event_b.wait_async(port, key, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_ONCE));

  std::atomic<bool> proceed{false};

  std::thread closer_threads[10];
  for (int t = 0; t < 10; t++) {
    closer_threads[t] = std::thread([&, t]() {
      while (!proceed.load()) {
        std::this_thread::yield();
      }
      for (int j = 0; j < t * 100; j++) {
        __asm__ volatile("" : : : "memory");
      }
      event_b.reset();
    });
  }

  proceed.store(true);
  zx_status_t status = port.cancel_key(0u, key);

  for (auto& closer_thread : closer_threads) {
    closer_thread.join();
  }

  EXPECT_TRUE(status == ZX_OK || status == ZX_ERR_NOT_FOUND);
}

TEST(PortTest, CreateInvalidOptions) {
  zx::port port;
  // Options other than 0 and ZX_PORT_BIND_TO_INTERRUPT should return ZX_ERR_INVALID_ARGS.
  EXPECT_EQ(zx::port::create(0x1234u, &port), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx::port::create(2u, &port), ZX_ERR_INVALID_ARGS);
}

TEST(PortTest, QueueWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  constexpr zx_port_packet_t packet = {1ull, ZX_PKT_TYPE_USER, 0, {{}}};
  // Passing a non-port handle to zx_port_queue returns ZX_ERR_WRONG_TYPE.
  EXPECT_EQ(zx_port_queue(event.get(), &packet), ZX_ERR_WRONG_TYPE);
}

TEST(PortTest, QueueAccessDenied) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  // Remove ZX_RIGHT_WRITE
  zx::port reduced_port;
  ASSERT_OK(port.replace(ZX_DEFAULT_PORT_RIGHTS & ~ZX_RIGHT_WRITE, &reduced_port));

  constexpr zx_port_packet_t packet = {1ull, ZX_PKT_TYPE_USER, 0, {{}}};
  EXPECT_EQ(reduced_port.queue(&packet), ZX_ERR_ACCESS_DENIED);
}

TEST(PortTest, WaitWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_port_packet_t packet = {};
  // Passing a non-port handle to zx_port_wait returns ZX_ERR_WRONG_TYPE.
  EXPECT_EQ(zx_port_wait(event.get(), ZX_TIME_INFINITE, &packet), ZX_ERR_WRONG_TYPE);
}

TEST(PortTest, WaitAccessDenied) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  // Remove ZX_RIGHT_READ
  zx::port reduced_port;
  ASSERT_OK(port.replace(ZX_DEFAULT_PORT_RIGHTS & ~ZX_RIGHT_READ, &reduced_port));

  zx_port_packet_t packet = {};
  EXPECT_EQ(reduced_port.wait(zx::time::infinite(), &packet), ZX_ERR_ACCESS_DENIED);
}

TEST(PortTest, WaitNullPacket) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  // Queue a packet first so zx_port_wait returns immediately instead of blocking,
  // and copy_to_user fails with ZX_ERR_INVALID_ARGS.
  constexpr zx_port_packet_t packet = {1ull, ZX_PKT_TYPE_USER, 0, {{}}};
  ASSERT_OK(port.queue(&packet));

  // Passing null packet pointer returns ZX_ERR_INVALID_ARGS.
  EXPECT_EQ(zx_port_wait(port.get(), ZX_TIME_INFINITE, nullptr), ZX_ERR_INVALID_ARGS);
}

TEST(PortTest, CancelWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx::event source;
  ASSERT_OK(zx::event::create(0u, &source));

  // Passing non-port handle to zx_port_cancel returns ZX_ERR_WRONG_TYPE.
  EXPECT_EQ(zx_port_cancel(event.get(), source.get(), 0u), ZX_ERR_WRONG_TYPE);
}

TEST(PortTest, CancelAccessDenied) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  // 1. Port lacks WRITE
  zx::port reduced_port;
  ASSERT_OK(port.duplicate(ZX_DEFAULT_PORT_RIGHTS & ~ZX_RIGHT_WRITE, &reduced_port));
  EXPECT_EQ(reduced_port.cancel(event, 0u), ZX_ERR_ACCESS_DENIED);

  // 2. Source lacks WAIT
  zx::event reduced_event;
  ASSERT_OK(event.duplicate(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_WAIT, &reduced_event));
  EXPECT_EQ(port.cancel(reduced_event, 0u), ZX_ERR_ACCESS_DENIED);
}

TEST(PortTest, CancelBadSourceHandle) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  // Passing ZX_HANDLE_INVALID returns ZX_ERR_BAD_HANDLE.
  EXPECT_EQ(port.cancel(*zx::unowned_event(ZX_HANDLE_INVALID), 0u), ZX_ERR_BAD_HANDLE);
}

TEST(PortTest, CancelKeyWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  // Passing non-port handle returns ZX_ERR_WRONG_TYPE.
  EXPECT_EQ(zx_port_cancel_key(event.get(), 0u, 0u), ZX_ERR_WRONG_TYPE);
}

TEST(PortTest, CancelKeyAccessDenied) {
  zx::port port;
  ASSERT_OK(zx::port::create(0u, &port));

  // Remove ZX_RIGHT_WRITE
  zx::port reduced_port;
  ASSERT_OK(port.replace(ZX_DEFAULT_PORT_RIGHTS & ~ZX_RIGHT_WRITE, &reduced_port));

  EXPECT_EQ(reduced_port.cancel_key(0u, 0u), ZX_ERR_ACCESS_DENIED);
}

TEST(PortTest, WaitAsyncPortWrongType) {
  zx::event event, not_a_port;
  ASSERT_OK(zx::event::create(0u, &event));
  ASSERT_OK(zx::event::create(0u, &not_a_port));

  // Passing a non-port handle as the port argument returns ZX_ERR_WRONG_TYPE.
  EXPECT_EQ(event.wait_async(*zx::unowned_port(not_a_port.get()), 1u, ZX_EVENT_SIGNALED, 0u),
            ZX_ERR_WRONG_TYPE);
}

TEST(PortTest, WaitAsyncPortAccessDenied) {
  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  // Remove ZX_RIGHT_WRITE from the port handle.
  zx::port reduced_port;
  ASSERT_OK(port.replace(ZX_DEFAULT_PORT_RIGHTS & ~ZX_RIGHT_WRITE, &reduced_port));

  EXPECT_EQ(event.wait_async(reduced_port, 1u, ZX_EVENT_SIGNALED, 0u), ZX_ERR_ACCESS_DENIED);
}

TEST(PortTest, WaitAsyncTargetAccessDenied) {
  zx::port port;
  zx::event event;
  ASSERT_OK(zx::port::create(0u, &port));
  ASSERT_OK(zx::event::create(0u, &event));

  // Remove ZX_RIGHT_WAIT from the event handle.
  zx::event reduced_event;
  ASSERT_OK(event.replace(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_WAIT, &reduced_event));

  EXPECT_EQ(reduced_event.wait_async(port, 1u, ZX_EVENT_SIGNALED, 0u), ZX_ERR_ACCESS_DENIED);
}

// Regression test for https://fxbug.dev/540007680
TEST(PortStressTest, CancelKeyDestructorReentersPortLock) {
  constexpr uint64_t kChannelKey = 0xC0FFEEull;
  // Must differ from kChannelKey: these observers have to SURVIVE the cancel_key
  // so that they are still registered when ~ChannelDispatcher cancels them.
  constexpr uint64_t kEventKey = 0xBEEFull;

  constexpr size_t kUnitsPerRound = 64;
  constexpr int kRounds = 16;
  constexpr int kClosers = 4;

  for (int round = 0; round < kRounds; ++round) {
    zx::port port;
    ASSERT_OK(zx::port::create(0u, &port));

    std::vector<zx::channel> targets;
    targets.reserve(kUnitsPerRound);

    for (size_t i = 0; i < kUnitsPerRound; ++i) {
      // (1) An event carrying a live observer on `port`. Never signalled, so
      //     Dispatcher::AddObserver takes the "no active match" path and the
      //     observer stays in the event's observer list.
      zx::event ev;
      ASSERT_OK(zx::event::create(0u, &ev));
      ASSERT_OK(ev.wait_async(port, kEventKey, ZX_EVENT_SIGNALED, ZX_WAIT_ASYNC_ONCE));

      // (2) Move the event's Handle into a channel message. The Handle object
      //     survives inside the queued MessagePacket and its observer is NOT
      //     cancelled by write (see fact 1 above).
      zx::channel producer, target;
      ASSERT_OK(zx::channel::create(0u, &producer, &target));
      zx_handle_t raw_ev = ev.release();
      ASSERT_OK(producer.write(0u, nullptr, 0u, &raw_ev, 1u));

      // Drop the producer end. The message, and the event handle it owns, stay
      // queued on `target` until target's ChannelDispatcher is destroyed.
      producer.reset();

      // (3) The observer we will cancel. ZX_USER_SIGNAL_0 is never asserted on
      //     a channel unless we assert it, so this observer is still registered
      //     at cancel_key time. (ZX_CHANNEL_READABLE would match immediately,
      //     a message is queued and the observer would be consumed before we
      //     ever got to cancel it.)
      ASSERT_OK(target.wait_async(port, kChannelKey, ZX_USER_SIGNAL_0, ZX_WAIT_ASYNC_ONCE));

      targets.push_back(std::move(target));
    }

    // The race. cancel_key walks every kChannelKey observer and, for each,
    // releases its keep-alive reference at the end of the CallUnlocked
    // statement, with the port lock back on. The closers try to make that
    // reference the last one.
    std::atomic<bool> go{false};
    std::atomic<size_t> next{0};

    std::thread closers[kClosers];
    for (auto& closer : closers) {
      closer = std::thread([&]() {
        while (!go.load(std::memory_order_acquire)) {
          std::this_thread::yield();
        }
        for (;;) {
          const size_t i = next.fetch_add(1, std::memory_order_relaxed);
          if (i >= targets.size()) {
            return;
          }
          targets[i].reset();
        }
      });
    }

    go.store(true, std::memory_order_release);
    const zx_status_t status = port.cancel_key(0u, kChannelKey);

    for (auto& closer : closers) {
      closer.join();
    }

    // ZX_ERR_NOT_FOUND simply means the closers won every slot this round.
    EXPECT_TRUE(status == ZX_OK || status == ZX_ERR_NOT_FOUND);
  }
}

}  // namespace
