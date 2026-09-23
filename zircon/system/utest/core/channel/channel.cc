// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Needed to test API coverage of null params in GCC.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnonnull"
#include <lib/zx/channel.h>
#pragma GCC diagnostic pop

#include <lib/arch/intrin.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/zx/event.h>
#include <lib/zx/fifo.h>
#include <lib/zx/job.h>
#include <lib/zx/object.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/socket.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/policy.h>
#include <zircon/types.h>

#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <set>
#include <thread>
#include <vector>

#include <mini-process/mini-process.h>
#include <zxtest/zxtest.h>

namespace channel {
namespace {

// Data used for writing into a channel.
constexpr uint32_t kChannelData = 0xdeadbeef;

TEST(ChannelTest, CreateIsOkAndEndpointsAreRelated) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx_info_handle_basic_t info[2];
  ASSERT_OK(local.get_info(ZX_INFO_HANDLE_BASIC, &info[0], sizeof(zx_info_handle_basic_t), nullptr,
                           nullptr));
  ASSERT_OK(remote.get_info(ZX_INFO_HANDLE_BASIC, &info[1], sizeof(zx_info_handle_basic_t), nullptr,
                            nullptr));
  ASSERT_NE(info[0].koid, 0);
  ASSERT_NE(info[1].koid, 0);

  EXPECT_EQ(info[0].related_koid, info[1].koid);
  EXPECT_EQ(info[1].related_koid, info[0].koid);
}

TEST(ChannelTest, IsWriteableByDefault) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx_signals_t local_pending = 0;
  zx_signals_t remote_pending = 0;
  ASSERT_OK(local.wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite_past(), &local_pending));
  ASSERT_OK(remote.wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite_past(), &remote_pending));
  EXPECT_EQ(ZX_CHANNEL_WRITABLE, local_pending);
  EXPECT_EQ(ZX_CHANNEL_WRITABLE, remote_pending);
}

TEST(ChannelTest, WriteToEndpointCausesOtherToBecomeReadable) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  ASSERT_OK(local.write(0u, &kChannelData, sizeof(uint32_t), nullptr, 0u));

  zx_signals_t local_pending = 0;
  zx_signals_t remote_pending = 0;
  ASSERT_OK(local.wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite_past(), &local_pending));
  ASSERT_OK(remote.wait_one(ZX_CHANNEL_WRITABLE | ZX_CHANNEL_READABLE, zx::time::infinite_past(),
                            &remote_pending));

  EXPECT_EQ(ZX_CHANNEL_WRITABLE, local_pending);
  EXPECT_EQ(ZX_CHANNEL_WRITABLE | ZX_CHANNEL_READABLE, remote_pending);
}

TEST(ChannelTest, WriteConsumesAllHandles) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  constexpr uint32_t kHandleCount = ZX_CHANNEL_MAX_MSG_HANDLES + 1;
  std::vector<zx::event> safe_handles(kHandleCount);
  for (uint32_t j = 0; j < kHandleCount; ++j) {
    ASSERT_OK(zx::event::create(0, &safe_handles[j]));
  }

  zx_handle_t handles[kHandleCount];
  for (uint32_t j = 0; j < kHandleCount; ++j) {
    handles[j] = safe_handles[j].release();
  }

  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, local.write(0, nullptr, 0, handles, kHandleCount));

  for (uint32_t j = 0; j < kHandleCount; ++j) {
    EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(handles[j]));
  }
}

// Regression test for https://fxbug.dev/467142666.
TEST(ChannelWriteTest, MoveHandlesWithRightsCheckFailure) {
  zx::channel channel_local, channel_remote;
  zx::event event, event_dup_no_transfer;

  ASSERT_OK(zx::channel::create(0, &channel_local, &channel_remote));
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(
      event.duplicate(ZX_DEFAULT_EVENT_RIGHTS & (~ZX_RIGHT_TRANSFER), &event_dup_no_transfer));

  zx_handle_t send_handle_list[] = {
      // This entry should transfer just fine.
      event.release(),
      // This entry should fail to transfer since this handle lacks ZX_RIGHT_TRANSFER.
      event_dup_no_transfer.release(),
  };

  // Without the fix for https://fxbug.dev/467142666 the kernel would identify
  // the handle transfer failure and then erroneously attempt to close all
  // handles in send_handle_list including the entry for |event| after removing
  // it from the handle table.
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            channel_local.write(0, &kChannelData, sizeof(kChannelData), send_handle_list,
                                std::size(send_handle_list)));

  // Both handles should be removed from our handle table even though the operation failed.
  EXPECT_EQ(ZX_ERR_NOT_FOUND, zx_handle_check_valid(send_handle_list[0]));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, zx_handle_check_valid(send_handle_list[1]));
}

enum class WorkerCompleteStatus : int {
  kSuccess = 0,
  kWaitError = 1,
  kReadFrom1Error = 2,
  kReadFrom2Error = 3,
  kDataMismatchFrom1Error = 4,
  kDataMismatchFrom2Error = 5,
};

void WaitOnChannels(zx::unowned_channel remote_1, zx::unowned_channel remote_2,
                    zx::unowned_event event, std::atomic<uint32_t>* total_packets,
                    std::atomic<uint32_t>* received_bytes_1,
                    std::atomic<uint32_t>* received_bytes_2,
                    std::atomic<WorkerCompleteStatus>* result) {
  zx_wait_item_t items[2];
  items[0].handle = remote_1->get();
  items[0].waitfor = ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED;

  items[1].handle = remote_2->get();
  items[1].waitfor = ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED;

  bool closed_1 = false;
  bool closed_2 = false;
  while (!closed_1 || !closed_2) {
    uint32_t data = 0u;
    uint32_t actual_bytes = 0u;
    if (zx::channel::wait_many(items, 2, zx::deadline_after(zx::duration::infinite())) != ZX_OK) {
      *result = WorkerCompleteStatus::kWaitError;
      return;
    }
    if (items[0].pending & ZX_CHANNEL_READABLE) {
      event->signal(0, ZX_USER_SIGNAL_0);
      if (remote_1->read(0u, &data, nullptr, sizeof(uint32_t), 0, &actual_bytes, nullptr) !=
          ZX_OK) {
        *result = WorkerCompleteStatus::kReadFrom1Error;
        return;
      }
      if (data != kChannelData) {
        *result = WorkerCompleteStatus::kDataMismatchFrom1Error;
        return;
      }
      *received_bytes_1 += actual_bytes;
      (*total_packets)++;
    } else if (items[1].pending & ZX_CHANNEL_READABLE) {
      event->signal(0, ZX_USER_SIGNAL_1);
      if (remote_2->read(0u, &data, nullptr, sizeof(uint32_t), 0, &actual_bytes, nullptr) !=
          ZX_OK) {
        *result = WorkerCompleteStatus::kReadFrom2Error;
        return;
      }
      if (data != kChannelData) {
        *result = WorkerCompleteStatus::kDataMismatchFrom2Error;
        return;
      }
      *received_bytes_2 += actual_bytes;
      (*total_packets)++;
    } else {
      if (items[0].pending & ZX_CHANNEL_PEER_CLOSED) {
        closed_1 = true;
      }
      if (items[1].pending & ZX_CHANNEL_PEER_CLOSED) {
        closed_2 = true;
      }
    }
  }

  *result = WorkerCompleteStatus::kSuccess;
  return;
}

TEST(ChannelTest, WaitManyIsSignaledOnAnyElementWrite) {
  zx::channel local_1, local_2;
  zx::channel remote_1, remote_2;

  ASSERT_OK(zx::channel::create(0, &local_1, &remote_1));
  ASSERT_OK(zx::channel::create(0, &local_2, &remote_2));
  std::atomic<uint32_t> received_packets = 0;
  std::atomic<uint32_t> received_bytes_1 = 0;
  std::atomic<uint32_t> received_bytes_2 = 0;
  std::atomic<WorkerCompleteStatus> result = WorkerCompleteStatus::kSuccess;
  zx::event event;

  ASSERT_OK(zx::event::create(0, &event));

  {
    std::jthread worker(&WaitOnChannels, zx::unowned_channel(remote_1),
                        zx::unowned_channel(remote_2), zx::unowned_event(event), &received_packets,
                        &received_bytes_1, &received_bytes_2, &result);
    // On exit close the local handles to unblock the service thread.
    auto cleanup = fit::defer([&local_1, &local_2]() {
      local_1.reset();
      local_2.reset();
    });
    ASSERT_OK(local_1.write(0, &kChannelData, sizeof(uint32_t), nullptr, 0));
    // We should expect only to be signalled for reading from remote_1.
    ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));
  }

  zx_signals_t event_signal;
  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1, zx::time::infinite_past(),
                           &event_signal));
  zx_signals_t signal_1;
  ASSERT_EQ(remote_1.wait_one(0, zx::time::infinite_past(), &signal_1), ZX_ERR_TIMED_OUT);
  zx_signals_t signal_2;
  ASSERT_EQ(remote_2.wait_one(0, zx::time::infinite_past(), &signal_2), ZX_ERR_TIMED_OUT);

  ASSERT_EQ(result, WorkerCompleteStatus::kSuccess);
  ASSERT_EQ(ZX_USER_SIGNAL_0, event_signal);
  EXPECT_EQ(ZX_CHANNEL_PEER_CLOSED, signal_1);
  EXPECT_EQ(ZX_CHANNEL_PEER_CLOSED, signal_2);
  EXPECT_EQ(received_bytes_1.load(), 1 * sizeof(uint32_t));
  EXPECT_EQ(received_bytes_2.load(), 0);
  EXPECT_EQ(received_packets, 1u);
}

TEST(ChannelTest, WaitManyIsSignaledForBothWrites) {
  zx::channel local_1, local_2;
  zx::channel remote_1, remote_2;

  ASSERT_OK(zx::channel::create(0, &local_1, &remote_1));
  ASSERT_OK(zx::channel::create(0, &local_2, &remote_2));
  std::atomic<uint32_t> received_packets = 0;
  std::atomic<uint32_t> received_bytes_1 = 0;
  std::atomic<uint32_t> received_bytes_2 = 0;
  std::atomic<WorkerCompleteStatus> result = WorkerCompleteStatus::kSuccess;
  zx::event event;

  ASSERT_OK(zx::event::create(0, &event));

  {
    std::jthread worker(&WaitOnChannels, zx::unowned_channel(remote_1),
                        zx::unowned_channel(remote_2), zx::unowned_event(event), &received_packets,
                        &received_bytes_1, &received_bytes_2, &result);
    // On exit close the local handles to unblock the service thread.
    auto cleanup = fit::defer([&local_1, &local_2]() {
      local_1.reset();
      local_2.reset();
    });
    ASSERT_OK(local_2.write(0, &kChannelData, sizeof(uint32_t), nullptr, 0));
    ASSERT_OK(local_1.write(0, &kChannelData, sizeof(uint32_t), nullptr, 0));
    // We should expect only to be signalled for reading from remote_1.
    ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));
    ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_1, zx::time::infinite(), nullptr));
  }

  zx_signals_t event_signal;
  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1, zx::time::infinite_past(),
                           &event_signal));
  zx_signals_t signal_1;
  ASSERT_EQ(remote_1.wait_one(0, zx::time::infinite_past(), &signal_1), ZX_ERR_TIMED_OUT);
  zx_signals_t signal_2;
  ASSERT_EQ(remote_2.wait_one(0, zx::time::infinite_past(), &signal_2), ZX_ERR_TIMED_OUT);

  ASSERT_EQ(result, WorkerCompleteStatus::kSuccess);
  ASSERT_EQ(ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1, event_signal);
  EXPECT_EQ(ZX_CHANNEL_PEER_CLOSED, signal_1);
  EXPECT_EQ(ZX_CHANNEL_PEER_CLOSED, signal_2);
  EXPECT_EQ(received_bytes_1.load(), 1 * sizeof(uint32_t));
  EXPECT_EQ(received_bytes_2.load(), 1 * sizeof(uint32_t));
  EXPECT_EQ(received_packets, 2u);
}

TEST(ChannelTest, ReadWhenEmptyReturnsShouldWait) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  ASSERT_EQ(remote.read(0, nullptr, nullptr, 0, 0, nullptr, nullptr), ZX_ERR_SHOULD_WAIT);
}

TEST(ChannelTest, ReadWhenEmptyAndClosedReturnsPeerClosed) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  local.reset();
  ASSERT_EQ(remote.read(0, nullptr, nullptr, 0, 0, nullptr, nullptr), ZX_ERR_PEER_CLOSED);
}

TEST(ChannelTest, ReadRemainingMessagesWhenPeerIsClosed) {
  constexpr uint32_t kMessageCount = 4;
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  for (uint32_t i = 0; i < kMessageCount; ++i) {
    ASSERT_OK(local.write(0, &kChannelData, sizeof(uint32_t), nullptr, 0));
  }

  local.reset();

  zx_signals_t signal;
  ASSERT_EQ(remote.wait_one(0, zx::time::infinite_past(), &signal), ZX_ERR_TIMED_OUT);
  ASSERT_EQ(ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, signal);

  for (uint32_t i = 0; i < kMessageCount; ++i) {
    uint32_t data;
    uint32_t read_bytes;
    ASSERT_OK(remote.read(0, &data, nullptr, sizeof(uint32_t), 0, &read_bytes, nullptr));
    ASSERT_EQ(sizeof(uint32_t), read_bytes);
    ASSERT_EQ(kChannelData, data);
  }
  // The channel should not be readable, since there are no remaining messages on it.
  ASSERT_EQ(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr),
            ZX_ERR_TIMED_OUT);
}

TEST(ChannelTest, CloseSignalsPeerClosed) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  local.reset();

  zx_signals_t signal;
  ASSERT_OK(remote.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &signal));
  EXPECT_TRUE(signal & ZX_CHANNEL_PEER_CLOSED);
}

TEST(ChannelTest, CloseClearsSignalsWriteable) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx_signals_t signal;
  ASSERT_EQ(remote.wait_one(0, zx::time::infinite_past(), &signal), ZX_ERR_TIMED_OUT);
  ASSERT_TRUE(signal & ZX_CHANNEL_WRITABLE);

  local.reset();

  ASSERT_OK(remote.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &signal));
  EXPECT_FALSE(signal & ZX_CHANNEL_WRITABLE);
}

TEST(ChannelTest, CloseSignalsPeerReturnsPeerClosed) {
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  local.reset();
  ASSERT_EQ(remote.signal_peer(0, ZX_USER_SIGNAL_0), ZX_ERR_PEER_CLOSED);
}

TEST(ChannelTest, OnFlightHandlesSignalledWhenPeerIsClosed) {
  zx::channel local;
  zx::channel remote;
  zx::channel on_flight_local[2];
  zx::channel on_flight_remote[2];
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  ASSERT_OK(zx::channel::create(0, &on_flight_local[0], &on_flight_remote[0]));
  ASSERT_OK(zx::channel::create(0, &on_flight_local[1], &on_flight_remote[1]));

  // Write each handle to the respective channel peer.
  zx_handle_t transferred = on_flight_remote[0].release();
  ASSERT_OK(local.write(0, nullptr, 0, &transferred, 1));

  transferred = on_flight_remote[1].release();
  ASSERT_OK(remote.write(0, nullptr, 0, &transferred, 1));

  // When the peer is closed, all unread handles should be closed.
  local.reset();

  // Now the local end of each transferred channel should be signalled.
  ASSERT_OK(on_flight_local[1].wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr));
  // Because |remote| is still not closed, then we can still read the remote end of the channel,
  // this should still be writeable.
  zx_signals_t signals;
  ASSERT_EQ(on_flight_local[0].wait_one(0, zx::time::infinite_past(), &signals), ZX_ERR_TIMED_OUT);
  ASSERT_NE(signals & ZX_CHANNEL_WRITABLE, 0);

  remote.reset();
  ASSERT_OK(on_flight_local[0].wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr));
  ASSERT_EQ(on_flight_local[1].wait_one(ZX_ERR_PEER_CLOSED, zx::time::infinite_past(), nullptr),
            ZX_ERR_TIMED_OUT);
}

TEST(ChannelTest, WriteNonTransferableHandleReturnsAccessDeniedAndClosesHandle) {
  zx::channel local;
  zx::channel remote;
  zx::event event;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  ASSERT_OK(zx::event::create(0, &event));

  zx_info_handle_basic_t event_info;
  ASSERT_OK(event.get_info(ZX_INFO_HANDLE_BASIC, &event_info, sizeof(zx_info_handle_basic_t),
                           nullptr, nullptr));

  // Remove the transfer right.
  zx_rights_t rights = event_info.rights & ~ZX_RIGHT_TRANSFER;
  zx::event non_transferable_event;
  ASSERT_OK(event.duplicate(rights, &non_transferable_event));

  zx_handle_t transferred = non_transferable_event.release();
  ASSERT_EQ(local.write(0, nullptr, 0, &transferred, 1), ZX_ERR_ACCESS_DENIED);

// Disable clang static analyzer for this line because it is testing handle
// double release intentionally.
#ifndef __clang_analyzer__
  ASSERT_EQ(zx_handle_close(transferred), ZX_ERR_BAD_HANDLE);
#endif  //  #ifndef __clang_analyzer__
}

TEST(ChannelTest, WriteRepeatedHandlesReturnsBadHandlesAndClosesHandle) {
  zx::channel local;
  zx::channel remote;
  zx::event event;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  ASSERT_OK(zx::event::create(0, &event));

  zx_handle_t event_handle = event.release();
  zx_handle_t handles[2] = {event_handle, event_handle};

  ASSERT_EQ(local.write(0, nullptr, 0, handles, 2), ZX_ERR_BAD_HANDLE);
// Disable clang static analyzer for this line because it is testing handle
// double release intentionally.
#ifndef __clang_analyzer__
  ASSERT_EQ(zx_handle_close(event_handle), ZX_ERR_BAD_HANDLE);
#endif  //  #ifndef __clang_analyzer__
}

TEST(ChannelTest, ConcurrentReadsConsumeUniqueElements) {
  zx::channel local;
  zx::channel remote;
  // Used to force both threads to stall until both are ready to run.
  zx::event event;

  // This number was 5000 but that triggers an ad-hoc policy exception on the number
  // of pending messages a channel can have.
  constexpr uint32_t kNumMessages = 2000;
  enum class ReadMessageStatus {
    kUnset,
    kReadFailed,
    kOk,
  };

  struct Message {
    uint64_t data = 0;
    uint32_t data_size = 0;
    ReadMessageStatus status = ReadMessageStatus::kUnset;
  };

  std::vector<Message> read_messages;
  read_messages.resize(kNumMessages);

  auto reader_worker = [&read_messages, &event, &remote](uint32_t offset) {
    zx_status_t wait_status = event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr);
    if (wait_status != ZX_OK) {
      return;
    }
    for (uint32_t i = 0; i < kNumMessages / 2; ++i) {
      uint64_t data = 0;
      uint32_t read_bytes = 0;
      zx_status_t read_status =
          remote.read(0, &data, nullptr, sizeof(uint64_t), 0, &read_bytes, nullptr);
      uint32_t index = offset + i;
      auto& message = read_messages[index];
      if (read_status != ZX_OK) {
        message.status = ReadMessageStatus::kReadFailed;
        continue;
      }
      message.status = ReadMessageStatus::kOk;
      message.data = data;
      message.data_size = read_bytes;
    }
    return;
  };

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  ASSERT_OK(zx::event::create(0, &event));
  constexpr uint32_t kReader1Offset = 0;
  constexpr uint32_t kReader2Offset = kNumMessages / 2;
  {
    auto cleanup = fit::defer([&local, &event]() {
      // Unlock read.
      local.reset();
      // Notify cancelled.
      event.reset();
    });
    {
      // These will be joined at the end of the inner block, before cleanup.
      std::jthread worker_1(reader_worker, kReader1Offset);
      std::jthread worker_2(reader_worker, kReader2Offset);

      for (uint64_t i = 1; i <= kNumMessages; ++i) {
        ASSERT_OK(local.write(0, &i, sizeof(uint64_t), nullptr, 0));
      }

      ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
    }
  }

  std::set<uint64_t> read_data;
  // Check that data os within (0, kNumMessages] range and that is monotonically increasing per
  // each reader.
  auto ValidateMessages = [&read_data, &read_messages, kNumMessages](uint32_t offset) {
    uint64_t prev = 0;
    for (uint32_t i = offset; i < kNumMessages / 2 + offset; ++i) {
      const auto& message = read_messages[i];
      read_data.insert(message.data);
      EXPECT_GT(message.data, 0);
      EXPECT_LE(message.data, kNumMessages);
      EXPECT_GT(message.data, prev);
      prev = message.data;
      EXPECT_EQ(message.data_size, sizeof(uint64_t));
      EXPECT_EQ(message.status, ReadMessageStatus::kOk);
    }
  };
  ValidateMessages(kReader1Offset);
  ValidateMessages(kReader2Offset);

  // No repeated messages.
  ASSERT_EQ(read_data.size(), kNumMessages,
            "Read messages do not match the number of written messages.");
}

constexpr uint32_t kMaxDataSize = 1000;
constexpr uint32_t kMaxHandleCount = 10;
constexpr char kEmptyData[kMaxDataSize] = {};

// Writes |msg_size| zeroed bytes to |channel| and |handle_count| duplicates of |event| to
// |channel|.
void WriteDataAndHandles(const zx::channel& channel, const zx::event& event, uint32_t msg_size,
                         uint32_t handle_count) {
  zx::event duplicates[kMaxHandleCount] = {};
  zx_handle_t handles[kMaxHandleCount] = {};

  ASSERT_LE(msg_size, kMaxDataSize);
  ASSERT_LE(handle_count, kMaxHandleCount);

  for (uint32_t i = 0; i < handle_count; ++i) {
    ASSERT_OK(event.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicates[i]));
  }

  // This is separate, so all duplicate handles are close if any duplication fails.
  for (uint32_t i = 0; i < handle_count; ++i) {
    handles[i] = duplicates[i].release();
  }

  ASSERT_OK(channel.write(0, kEmptyData, msg_size, handles, handle_count));
}

template <typename T>
void CheckHandleCount(const zx::object<T>& zx_object, uint32_t expected_count) {
  // Only the handle to |event| remains.
  zx_info_handle_count_t handle_info;
  ASSERT_OK(zx_object.get_info(ZX_INFO_HANDLE_COUNT, &handle_info, sizeof(zx_info_handle_count_t),
                               nullptr, nullptr));
  ASSERT_EQ(expected_count, handle_info.handle_count);
}

template <uint32_t ByteBufferSize, uint32_t HandleCount,
          bool convert_zero_elements_to_nullptr = false>
void PerformChannelCallWithSmallBuffer(const zx::channel& local, const zx::channel& remote,
                                       uint32_t reply_byte_size, uint32_t reply_handle_count,
                                       uint32_t* actual_bytes, uint32_t* actual_handles) {
  // An extra element to prevent 0 sized arrays.
  uint8_t buffer[ByteBufferSize + 1] = {};
  zx_handle_t handles[HandleCount + 1] = {};

  uint8_t* buffer_ptr = (convert_zero_elements_to_nullptr) ? nullptr : buffer;
  zx_handle_t* handles_ptr = (convert_zero_elements_to_nullptr) ? nullptr : handles;

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  ASSERT_EQ(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr),
            ZX_ERR_TIMED_OUT);
  ASSERT_NO_FATAL_FAILURE(WriteDataAndHandles(local, event, reply_byte_size, reply_handle_count));
  ASSERT_EQ(remote.read(ZX_CHANNEL_READ_MAY_DISCARD, buffer_ptr, handles_ptr, ByteBufferSize,
                        HandleCount, actual_bytes, actual_handles),
            ZX_ERR_BUFFER_TOO_SMALL);
  ASSERT_EQ(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr),
            ZX_ERR_TIMED_OUT);
  // At the end, only one handle should remain.
  ASSERT_NO_FATAL_FAILURE(CheckHandleCount(event, 1));
}

TEST(ChannelTest, ReadMayDiscardWithNullBuffersReturnsBufferTooSmall) {
  constexpr uint32_t kDataSize = 0;
  constexpr uint32_t kHandleCount = 0;

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  uint32_t actual_bytes = 0;
  uint32_t actual_handle_count = 0;

  ASSERT_NO_FATAL_FAILURE((PerformChannelCallWithSmallBuffer<kDataSize, kHandleCount, true>(
      local, remote, kDataSize + 1, kHandleCount + 1, &actual_bytes, &actual_handle_count)));

  EXPECT_EQ(kHandleCount + 1, actual_handle_count);
  EXPECT_EQ(kDataSize + 1, actual_bytes);
}

TEST(ChannelTest, ReadMayDiscardWithNullBufferDiscardsDataReturnsBufferTooSmall) {
  constexpr uint32_t kDataSize = 1;
  constexpr uint32_t kHandleCount = 0;

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  uint32_t actual_bytes = 0;
  uint32_t actual_handle_count = 0;

  ASSERT_NO_FATAL_FAILURE((PerformChannelCallWithSmallBuffer<kDataSize, kHandleCount, true>(
      local, remote, kDataSize + 1, kHandleCount, &actual_bytes, &actual_handle_count)));

  EXPECT_EQ(kHandleCount, actual_handle_count);
  EXPECT_EQ(kDataSize + 1, actual_bytes);
}

TEST(ChannelTest, ReadMayDiscardWithNullBufferDiscardHandlesReturnsBufferTooSmall) {
  constexpr uint32_t kDataSize = 0;
  constexpr uint32_t kHandleCount = 1;

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  uint32_t actual_bytes = 0;
  uint32_t actual_handle_count = 0;

  ASSERT_NO_FATAL_FAILURE((PerformChannelCallWithSmallBuffer<kDataSize, kHandleCount, true>(
      local, remote, kDataSize, kHandleCount + 1, &actual_bytes, &actual_handle_count)));

  EXPECT_EQ(kHandleCount + 1, actual_handle_count);
  EXPECT_EQ(kDataSize, actual_bytes);
}

TEST(ChannelTest, ReadMayDiscardWithZeroSizeBuffersDiscardHandlesAndDataReturnsBufferTooSmall) {
  constexpr uint32_t kDataSize = 0;
  constexpr uint32_t kHandleCount = 0;

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  uint32_t actual_bytes = 0;
  uint32_t actual_handle_count = 0;

  ASSERT_NO_FATAL_FAILURE((PerformChannelCallWithSmallBuffer<kDataSize, kHandleCount, true>(
      local, remote, kDataSize + 1, kHandleCount + 1, &actual_bytes, &actual_handle_count)));

  EXPECT_EQ(kHandleCount + 1, actual_handle_count);
  EXPECT_EQ(kDataSize + 1, actual_bytes);
}

TEST(ChannelTest, ReadMayDiscardWithSmallerBufferDiscardHandlesAndDateReturnsBufferTooSmall) {
  constexpr uint32_t kDataSize = 10;
  constexpr uint32_t kHandleCount = 1;

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  uint32_t actual_bytes = 0;
  uint32_t actual_handle_count = 0;

  ASSERT_NO_FATAL_FAILURE((PerformChannelCallWithSmallBuffer<kDataSize, kHandleCount>(
      local, remote, kDataSize + 1, kHandleCount + 1, &actual_bytes, &actual_handle_count)));

  EXPECT_EQ(kHandleCount + 1, actual_handle_count);
  EXPECT_EQ(kDataSize + 1, actual_bytes);
}

struct Message {
  static constexpr uint32_t kDataSize = 64;
  static constexpr uint32_t kHeaderSize = sizeof(zx_txid_t);
  static constexpr uint32_t kMaxSize = kDataSize + kHeaderSize;
  static constexpr uint32_t kHandleCount = 10;

  zx_status_t Write(const zx::channel& channel) {
    return channel.write(0, start(), byte_size(), handles, handle_count);
  }

  zx_status_t Read(const zx::channel& channel, uint32_t* actual_bytes = nullptr,
                   uint32_t* actual_handles = nullptr) {
    return channel.read(0, start(), handles, byte_size(), handle_count, actual_bytes,
                        actual_handles);
  }

  Message(uint32_t data_size = 0, uint32_t handle_count = 0) {
    this->data_size = data_size;
    this->handle_count = handle_count;
  }

  const uint8_t* start() const { return reinterpret_cast<const uint8_t*>(&id); }

  const uint8_t* end() const { return reinterpret_cast<const uint8_t*>(data) + data_size; }

  uint8_t* start() { return reinterpret_cast<uint8_t*>(&id); }

  uint8_t* end() { return reinterpret_cast<uint8_t*>(data) + data_size; }

  bool IsEquivalent(const Message& rhs) const {
    if (data_size != rhs.data_size) {
      return false;
    }

    if (memcmp(data, rhs.data, data_size) != 0) {
      return false;
    }

    if (handle_count != rhs.handle_count) {
      return false;
    }

    return true;
  }

  uint32_t byte_size() const { return static_cast<uint32_t>(end() - start()); }

  void CloseHandles() {
    for (uint32_t i = 0; i < handle_count; ++i) {
      zx_handle_close(handles[i]);
    }
  }

  zx_txid_t id = 0;
  uint32_t data[kDataSize] = {0};
  uint32_t data_size = kDataSize * sizeof(uint32_t);
  zx_handle_t handles[kHandleCount];
  uint32_t handle_count = 0;
};

TEST(ChannelTest, CallWrittenBytesSmallerThanZxTxIdReturnsInvalidArgs) {
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message request;

  Message reply;
  zx_channel_call_args_t args = {
      .wr_bytes = &request,
      .wr_handles = nullptr,
      .rd_bytes = &reply,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(zx_txid_t) - 1,
      .wr_num_handles = 0,
      .rd_num_bytes = Message::kMaxSize,
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

// Currently the check for sufficient space for txid happens after reading in message data.
// Test failing reading in message data before checking txid size.
TEST(ChannelTest, CallWrittenBytesSmallerThanZxTxIdWithBadPointerReturnsInvalidArgs) {
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message reply;
  zx_channel_call_args_t args = {
      .wr_bytes = reinterpret_cast<void*>(-1),  // Bad pointer.
      .wr_handles = nullptr,
      .rd_bytes = &reply,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(zx_txid_t) - 1,
      .wr_num_handles = 0,
      .rd_num_bytes = Message::kMaxSize,
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

template <auto ReplyFiller, uint32_t accumulated_messages = 0>
void ReplyAndWait(const Message& request, uint32_t message_count, zx::channel svc,
                  std::atomic<const char*>* error, zx::event* wait_for_event) {
  std::set<zx_txid_t> live_ids;
  std::vector<Message> live_requests;
  auto cleanup = fit::defer([&svc, &live_requests]() {
    svc.reset();
    for (auto req : live_requests) {
      for (uint32_t i = 0; i < req.handle_count; ++i) {
        zx_handle_close(req.handles[i]);
      }
    }
  });
  for (uint32_t i = 0; i < message_count; ++i) {
    svc.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr);
    Message read_request = Message(request.data_size, request.handle_count);
    if (read_request.Read(svc) != ZX_OK) {
      *error = "Failed to read request.";
      return;
    }
    if (!request.IsEquivalent(read_request)) {
      *error = "Failed to validate request.";
      return;
    }
    read_request.CloseHandles();

    if (i <= accumulated_messages) {
      if (live_ids.find(read_request.id) != live_ids.end()) {
        *error = "Repeated id used for live transaction.";
        return;
      }
      live_ids.insert(read_request.id);
      live_requests.push_back(read_request);
      if (i + 1 < accumulated_messages) {
        continue;
      }
    }

    // This is the last pending message, so we reply to all pending messages and then we reply.
    for (auto req : live_requests) {
      Message reply = Message(0, 0);
      reply.id = req.id;
      ReplyFiller(&reply);
      if (reply.Write(svc) != ZX_OK) {
        *error = "Failed to write reply.";
        return;
      }
    }
  }

  if (wait_for_event != nullptr) {
    if (wait_for_event->wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr) != ZX_OK) {
      *error = "Failed to wait for signal event.";
      return;
    }
  }
}

template <auto ReplyFiller, uint32_t accumulated_messages = 0>
void Reply(const Message& request, uint32_t message_count, zx::channel svc,
           std::atomic<const char*>* error) {
  ReplyAndWait<ReplyFiller, accumulated_messages>(request, message_count, std::move(svc), error,
                                                  nullptr);
}

zx_channel_call_args_t MakeArgs(const Message& request, Message* reply) {
  zx_channel_call_args_t args;
  args.wr_bytes = request.start();
  args.wr_handles = request.handles;
  args.wr_num_bytes = request.byte_size();
  args.wr_num_handles = request.handle_count;
  args.rd_bytes = reply->start();
  args.rd_handles = reply->handles;
  args.rd_num_bytes = reply->byte_size();
  args.rd_num_handles = reply->handle_count;
  return args;
}

template <int data_size, uint32_t handles>
void ReplyFiller(Message* reply) {
  reply->data_size = data_size;

  uint32_t i = 0;
  auto cleanup = fit::defer([&i, reply]() {
    for (uint32_t j = 0; j < i; ++j) {
      zx_handle_close(reply->handles[j]);
    }
  });

  reply->handle_count = handles;
  for (i = 0; i < handles; ++i) {
    zx::event event;
    if (zx::event::create(0, &event) != ZX_OK) {
      return;
    }
    reply->handles[i] = event.release();
  }
  cleanup.cancel();
}

TEST(ChannelTest, CallResponseBiggerThanRdNumBytesReturnsBufferTooSmall) {
  constexpr uint32_t kReplyDataSize = 2;
  constexpr uint32_t kReplyHandleCount = 0;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message request = Message(5 * sizeof(uint32_t), 0);
  request.id = 0x112233;
  request.data[0] = 1;
  request.data[1] = 2;
  request.data[2] = 3;
  request.data[3] = 4;
  request.data[4] = 5;

  Message reply = Message(kReplyDataSize - 1, kReplyHandleCount);
  auto args = MakeArgs(request, &reply);

  {
    std::jthread service_thread(Reply<ReplyFiller<kReplyDataSize, kReplyHandleCount>>, request, 1,
                                std::move(remote), &error);

    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_EQ(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
              ZX_ERR_BUFFER_TOO_SMALL);
  }

  reply.CloseHandles();
  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallResponseBiggerThanRdNumHandlesReturnsBufferTooSmall) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 2;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  Message request = Message(0, 1);
  request.id = 0x112233;
  request.handles[0] = event.release();

  Message reply = Message(0, kReplyHandleCount - 1);
  auto args = MakeArgs(request, &reply);

  {
    std::jthread service_thread(Reply<ReplyFiller<kReplyDataSize, kReplyHandleCount>>, request, 1,
                                std::move(remote), &error);
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_EQ(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
              ZX_ERR_BUFFER_TOO_SMALL);
  }
  reply.CloseHandles();

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

template <uint32_t ReplyDataSize, uint32_t ReplyHandleCount>
void SuccessfullChannelCall(zx::channel local, zx::channel remote, const Message& request) {
  std::atomic<const char*> error = nullptr;
  Message reply = Message(ReplyDataSize, ReplyHandleCount);

  auto args = MakeArgs(request, &reply);
  {
    std::jthread service_thread(Reply<ReplyFiller<ReplyDataSize, ReplyHandleCount>>, request, 1,
                                std::move(remote), &error);
    uint32_t hc, bc;
    ASSERT_OK(local.call(0, zx::time::infinite(), &args, &bc, &hc));
  }
  reply.CloseHandles();

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallBytesFitIsOk) {
  constexpr uint32_t kReplyDataSize = 5;
  constexpr uint32_t kReplyHandleCount = 0;

  Message request = Message(4, 0);

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  ASSERT_NO_FATAL_FAILURE((SuccessfullChannelCall<kReplyDataSize, kReplyHandleCount>(
      std::move(local), std::move(remote), request)));
}

TEST(ChannelTest, CallHandlesFitIsOk) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 2;

  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  Message request = Message(0, 1);
  request.handles[0] = event.release();
  ASSERT_NO_FATAL_FAILURE((SuccessfullChannelCall<kReplyDataSize, kReplyHandleCount>(
      std::move(local), std::move(remote), request)));
}

TEST(ChannelTest, CallHandleAndBytesFitsIsOk) {
  constexpr uint32_t kReplyDataSize = 2;
  constexpr uint32_t kReplyHandleCount = 2;

  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  Message request = Message(2, 1);
  request.handles[0] = event.release();

  ASSERT_NO_FATAL_FAILURE((SuccessfullChannelCall<kReplyDataSize, kReplyHandleCount>(
      std::move(local), std::move(remote), request)));
}

// UBSan was triggering on passing nullptr to zx_channel_call which doesn't
// accept null arguments. This is what this specific test is checking though, so
// we can just wrap the call to zx_channel_call() in a function that disables
// this UBSan check.
[[gnu::no_sanitize("undefined")]]
#ifndef __clang__
// Don't inline this so GCC doesn't see there is only one caller and it uses
// nullptr.
[[gnu::noinline]]
#endif
zx_status_t local_call(const zx::channel& local, zx_channel_call_args_t& args, uint32_t* bytes,
                       uint32_t* handles) {
  return zx_channel_call(local.get(), 0, zx::time::infinite().get(), &args, bytes, handles);
}

TEST(ChannelTest, CallNullptrNumBytesIsInvalidArgs) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 0;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message request = Message(2, 0);
  Message reply = Message(kReplyDataSize, kReplyHandleCount);
  auto args = MakeArgs(request, &reply);
  {
    std::jthread service_thread(Reply<ReplyFiller<kReplyDataSize, kReplyHandleCount>>, request, 1,
                                std::move(remote), &error);
    uint32_t hc;
    ASSERT_EQ(local_call(local, args, nullptr, &hc), ZX_ERR_INVALID_ARGS);
  }
  reply.CloseHandles();

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallNullptrNumHandlesInvalidArgs) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 0;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message request = Message(2, 0);
  Message reply = Message(kReplyDataSize, kReplyHandleCount);
  auto args = MakeArgs(request, &reply);
  {
    std::jthread service_thread(Reply<ReplyFiller<kReplyDataSize, kReplyHandleCount>>, request, 1,
                                std::move(remote), &error);
    uint32_t bc;
    ASSERT_EQ(local_call(local, args, &bc, nullptr), ZX_ERR_INVALID_ARGS);
  }
  reply.CloseHandles();

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallPendingTransactionsUseDifferentIds) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 0;
  // The service thread will wait until |kAcummulatedMessages| have been read from the channel
  // before replying in the same order they came through.
  constexpr uint32_t kAccumulatedMessages = 20;

  std::atomic<const char*> error = nullptr;
  std::vector<zx_status_t> call_result(kAccumulatedMessages, ZX_OK);
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message request = Message(2, 0);
  {
    std::jthread service_thread(
        Reply<ReplyFiller<kReplyDataSize, kReplyHandleCount>, kAccumulatedMessages>, request,
        kAccumulatedMessages, std::move(remote), &error);

    std::vector<std::jthread> calling_threads;
    calling_threads.reserve(kAccumulatedMessages);
    for (uint32_t i = 0; i < kAccumulatedMessages; ++i) {
      calling_threads.push_back(std::jthread([i, &call_result, &local, &request]() {
        Message reply = Message(kReplyDataSize, kReplyHandleCount);
        auto args = MakeArgs(request, &reply);
        uint32_t bc, hc;
        call_result[i] = local.call(0, zx::time::infinite(), &args, &bc, &hc);
        if (call_result[i] == ZX_OK) {
          reply.CloseHandles();
        }
      }));
    }
  }

  for (auto call_status : call_result) {
    EXPECT_OK(call_status, "channel::call failed in client thread.");
  }

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallDeadlineExceededReturnsTimedOut) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 0;
  constexpr uint32_t kAccumulatedMessages = 2;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  Message request = Message(2, 0);
  Message reply = Message(kReplyDataSize, kReplyHandleCount);
  auto args = MakeArgs(request, &reply);
  {
    std::jthread service_thread(
        ReplyAndWait<ReplyFiller<kReplyDataSize, kReplyHandleCount>, kAccumulatedMessages>, request,
        kAccumulatedMessages - 1, std::move(remote), &error, &event);
    uint32_t bc, hc;
    ASSERT_EQ(local.call(0, zx::time::infinite_past(), &args, &bc, &hc), ZX_ERR_TIMED_OUT);
    event.signal(0, ZX_USER_SIGNAL_0);
  }
  reply.CloseHandles();

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallConsumesHandlesOnSuccess) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 0;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx::event event_2;
  ASSERT_OK(zx::event::create(0, &event_2));

  Message request = Message(0, 2);
  request.handles[0] = event.release();
  request.handles[1] = event_2.release();

  Message reply = Message(kReplyDataSize, kReplyHandleCount);

  auto args = MakeArgs(request, &reply);
  {
    std::jthread service_thread(Reply<ReplyFiller<kReplyDataSize, kReplyHandleCount>>, request, 1,
                                std::move(remote), &error);
    uint32_t hc, bc;
    ASSERT_OK(local.call(0, zx::time::infinite(), &args, &bc, &hc));
  }

  reply.CloseHandles();

  for (uint32_t i = 0; i < request.handle_count; ++i) {
    ASSERT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(request.handles[i]));
  }

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallConsumesHandlesOnError) {
  constexpr uint32_t kReplyDataSize = 0;
  constexpr uint32_t kReplyHandleCount = 0;

  std::atomic<const char*> error = nullptr;
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));
  remote.reset();
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx::event event_2;
  ASSERT_OK(zx::event::create(0, &event_2));

  Message request = Message(0, 2);
  request.handles[0] = event.release();
  request.handles[1] = event_2.release();

  Message reply = Message(kReplyDataSize, kReplyHandleCount);

  auto args = MakeArgs(request, &reply);
  {
    uint32_t hc, bc;
    ASSERT_EQ(ZX_ERR_PEER_CLOSED, local.call(0, zx::time::infinite(), &args, &bc, &hc));
  }

  reply.CloseHandles();

  EXPECT_EQ(2, request.handle_count);
  for (uint32_t i = 0; i < request.handle_count; ++i) {
    ASSERT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(request.handles[i]));
  }

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelTest, CallNotifiedOnPeerClosed) {
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  Message request = Message(0, 0);
  Message reply = Message(0, 0);

  auto args = MakeArgs(request, &reply);
  {
    std::jthread service_thread(
        [](zx::channel svc) {
          // Wait until call message is received.
          svc.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr);
          svc.reset();
        },
        std::move(remote));

    uint32_t bc, hc;
    ASSERT_EQ(ZX_ERR_PEER_CLOSED, local.call(0, zx::time::infinite(), &args, &bc, &hc));
  }
}

// Nest 200 channels, each one in the payload of the previous one. Without
// the SafeDeleter in fbl_recycle() this blows the kernel stack when calling
// the destructors.
TEST(ChannelTest, NestingIsOk) {
  constexpr uint32_t kNestedCount = 200;
  std::vector<zx::channel> local(kNestedCount);
  std::vector<zx::channel> remote(kNestedCount);

  for (uint32_t i = 0; i < kNestedCount; ++i) {
    ASSERT_OK(zx::channel::create(0, &local[i], &remote[i]));
  }

  for (uint32_t i = kNestedCount - 1; i > 0; --i) {
    zx_handle_t handles[2] = {local[i].release(), remote[i].release()};
    ASSERT_OK(local[i - 1].write(0, nullptr, 0, handles, 2));
  }

  // All handles except those at 0, have been transferred to a channel.
  ASSERT_TRUE(local[0].is_valid());
  ASSERT_TRUE(remote[0].is_valid());

  // Close the handles and for destructions.
  local[0].reset();
  remote[0].reset();
}

TEST(ChannelTest, WriteSelfHandleReturnsNotSupported) {
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::unowned_channel unowned_local(local.get());
  zx_handle_t local_handle = local.release();
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, unowned_local->write(0, nullptr, 0, &local_handle, 1));

  zx_signals_t signals;
  ASSERT_OK(remote.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &signals));
  ASSERT_EQ(ZX_CHANNEL_PEER_CLOSED, signals);
}

TEST(ChannelTest, ReadEtcHandleInfoValidation) {
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Handles to send.
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx::event event_with_less_rights;
  ASSERT_OK(event.duplicate(ZX_RIGHTS_BASIC & ~ZX_RIGHT_WAIT, &event_with_less_rights));

  zx::fifo fifo_local, fifo_remote;
  ASSERT_OK(zx::fifo::create(32, 8, 0, &fifo_local, &fifo_remote));

  zx_handle_t handles[4] = {
      fifo_local.release(),
      event.release(),
      event_with_less_rights.release(),
      fifo_remote.release(),
  };

  ASSERT_OK(local.write(0, nullptr, 0, handles, 4));

  zx_handle_info_t read_handles[4] = {};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;

  ASSERT_OK(remote.read_etc(0, nullptr, read_handles, 0, 4, &actual_bytes, &actual_handles));

  ASSERT_EQ(4, actual_handles);
  ASSERT_EQ(0, actual_bytes);

  EXPECT_EQ(read_handles[0].type, ZX_OBJ_TYPE_FIFO);
  EXPECT_EQ(read_handles[0].rights, ZX_DEFAULT_FIFO_RIGHTS);

  EXPECT_EQ(read_handles[1].type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(read_handles[1].rights, ZX_DEFAULT_EVENT_RIGHTS);

  EXPECT_EQ(read_handles[2].type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(read_handles[2].rights, ZX_RIGHTS_BASIC & ~ZX_RIGHT_WAIT);

  EXPECT_EQ(read_handles[3].type, ZX_OBJ_TYPE_FIFO);
  EXPECT_EQ(read_handles[3].rights, ZX_DEFAULT_FIFO_RIGHTS);
}

TEST(ChannelTest, ReadAndWriteWithMultipleSizes) {
  zx::channel local;
  zx::channel remote;

  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Use the seed that was passed as cmd or generated by the library.
  unsigned int seed = zxtest::Runner::GetInstance()->random_seed();

  constexpr uint32_t kNumMessages = 1000;
  for (uint32_t i = 0; i < kNumMessages; ++i) {
    const uint32_t num_bytes = rand_r(&seed) % ZX_CHANNEL_MAX_MSG_BYTES;
    const uint32_t num_handles = rand_r(&seed) % ZX_CHANNEL_MAX_MSG_HANDLES;

    std::vector<uint8_t> data(num_bytes + 1, 0);
    std::vector<zx_handle_t> handles(num_handles + 1, ZX_HANDLE_INVALID);
    std::vector<zx::event> safe_handles(num_handles + 1);

    for (uint32_t j = 0; j < num_handles; ++j) {
      ASSERT_OK(zx::event::create(0, &safe_handles[j]));
    }

    data[0] = static_cast<uint8_t>(i % std::numeric_limits<uint8_t>::max());

    // Transfer handles
    for (uint32_t j = 0; j < num_handles; ++j) {
      handles[j] = safe_handles[j].release();
    }

    ASSERT_OK(local.write(0, data.data(), num_bytes, handles.data(), num_handles));

    std::vector<uint8_t> read_data(num_bytes + 1, 0);
    std::vector<zx_handle_t> read_handles(num_handles + 1, ZX_HANDLE_INVALID);
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;

    ASSERT_OK(remote.read(0, read_data.data(), read_handles.data(), num_bytes, num_handles,
                          &actual_bytes, &actual_handles));
    // Transfer handles to safe_handles so they are destroyed on destruction.
    for (uint32_t j = 0; j < num_handles; ++j) {
      safe_handles[j].reset(read_handles[j]);
    }
    ASSERT_EQ(num_bytes, actual_bytes);
    ASSERT_EQ(num_handles, actual_handles);
    if (num_bytes > 0) {
      ASSERT_EQ(data[0], read_data[0]);
    }
  }
}

// Verify that a process is killed by a policy exception if it writes to a "full" channel.
TEST(ChannelTest, ChannelFullException) {
  if (getenv("NO_NEW_PROCESS")) {
    ZXTEST_SKIP("Running without the ZX_POL_NEW_PROCESS policy, skipping test case.");
  }

  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  constexpr char kName[] = "channel-full-exception-test";
  ASSERT_OK(
      zx::process::create(*zx::job::default_job(), kName, sizeof(kName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kName, sizeof(kName) - 1, 0u, &thread));

  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::channel cmd_channel;
  ASSERT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), local.release(), true,
                                   cmd_channel.reset_and_get_address()));

  uint64_t write_count = 0;
  while (mini_process_cmd(cmd_channel.get(), MINIP_CMD_CHANNEL_WRITE, nullptr) == ZX_OK) {
    write_count++;
  }

  // Make sure it wrote at least a reasonable number of messages before terminating.
  ASSERT_GT(write_count, 1000);
  ASSERT_OK(proc.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), nullptr));

  zx_info_process_t proc_info;
  ASSERT_OK(proc.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr));
  ASSERT_EQ(proc_info.return_code, ZX_TASK_RETCODE_EXCEPTION_KILL);
}

// This is a regression test for https://fxbug.dev/42155375.
//
// Ensure that the kernel does not leak handle objects when copy out faults.
TEST(ChannelTest, ReadFaultDoesNotLeakHandles) {
  // Create a channel.
  zx::channel a;
  zx::channel b;
  ASSERT_OK(zx::channel::create(0, &a, &b));

  char msg{};

  // Obtain a invalid pointer.  When reading a we'll ask the kernel to place the handles here, which
  // will fail.  Ideally, to obtain an invalid pointer we'd map a single page VMO with no read/write
  // permissions.  However, that will generate a lot of log spam (`PageFault: error -30`).  Instead
  // provide a value that's in the kernel's section of the address space.  That way, the copy out
  // will fail early.
  static constexpr zx_vaddr_t kBadAddress = 0xfffffffffffffff0;

  // This value should be larger than the kernel's maximum number of handles to ensure that if we do
  // create a leak we will completely exhaust the handle arena.
  static constexpr size_t kCount = (256 + 1) * 1024;
  for (size_t i = 0; i < kCount; ++i) {
    zx::event event;
    // If the handle area is exhausted, we may see this call fail with ZX_ERR_NO_MEMORY.
    ASSERT_OK(zx::event::create(0, &event));
    zx_handle_t handle = event.release();
    a.write(0, &msg, sizeof(msg), &handle, 1);
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, b.read(0, &msg, reinterpret_cast<zx_handle_t*>(kBadAddress),
                                          sizeof(msg), 1, nullptr, nullptr));
  }
}

// This test verifies that when a channel reader+waiter races with a channel writer, the reader will
// never be told the channel is readable when it's not.
TEST(ChannelTest, NoSpuriousReadableSignalWhenRacing) {
  static constexpr size_t kAttempts = 10000;

  auto writer = [](std::atomic<bool>& running, std::atomic<uint64_t>& attempt, zx::channel b) {
    uint64_t curr;
    while (running.load() && (curr = attempt.load()) < kAttempts) {
      char msg[1]{};
      b.write(0, &msg, sizeof(msg), nullptr, 0);
      // Wait for the next attempt.
      while (running.load() && attempt.load() == curr) {
        arch::Yield();
      }
    }
  };

  std::atomic<bool> running{true};
  std::atomic<uint64_t> attempt{0};
  zx::channel a;
  zx::channel b;
  ASSERT_OK(zx::channel::create(0, &a, &b));
  std::thread w(writer, std::ref(running), std::ref(attempt), std::move(b));
  auto cleanup = fit::defer([&]() {
    running.store(false);
    w.join();
  });

  for (size_t x = 0; x < kAttempts; ++x) {
    attempt.store(x);
    char msg[1];
    zx_status_t status = a.read(0, &msg, nullptr, sizeof(msg), 0, nullptr, nullptr);
    if (status == ZX_ERR_SHOULD_WAIT) {
      ASSERT_OK(a.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
      ASSERT_OK(a.read(0, &msg, nullptr, sizeof(msg), 0, nullptr, nullptr));
    }
  }
  attempt.store(kAttempts);
}

TEST(ChannelWriteEtcTest, MoveSuccess) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t event_raw = event.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS,
      .result = ZX_OK,
  };

  ASSERT_OK(local.write_etc(0, nullptr, 0, &disp, 1));
  EXPECT_OK(disp.result);

  // Handle should be closed in sender.
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(event_raw));

  // Read from remote.
  zx_handle_info_t read_handle = {};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read_etc(0, nullptr, &read_handle, 0, 1, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_handles, 1);
  EXPECT_EQ(read_handle.type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(read_handle.rights, ZX_DEFAULT_EVENT_RIGHTS);

  zx_handle_close(read_handle.handle);
}

TEST(ChannelWriteEtcTest, DuplicateSuccess) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_DUPLICATE,
      .handle = event.get(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS,
      .result = ZX_OK,
  };

  ASSERT_OK(local.write_etc(0, nullptr, 0, &disp, 1));
  EXPECT_OK(disp.result);

  // Handle should still be valid in sender.
  EXPECT_OK(zx_handle_check_valid(event.get()));

  // Read from remote.
  zx_handle_info_t read_handle = {};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read_etc(0, nullptr, &read_handle, 0, 1, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_handles, 1);
  EXPECT_EQ(read_handle.type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(read_handle.rights, ZX_DEFAULT_EVENT_RIGHTS);

  zx_handle_close(read_handle.handle);
}

TEST(ChannelWriteEtcTest, ReducedRightsSuccess) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx_rights_t reduced_rights = ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_SIGNAL;

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = reduced_rights,
      .result = ZX_OK,
  };

  ASSERT_OK(local.write_etc(0, nullptr, 0, &disp, 1));
  EXPECT_OK(disp.result);

  // Read from remote.
  zx_handle_info_t read_handle = {};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read_etc(0, nullptr, &read_handle, 0, 1, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_handles, 1);
  EXPECT_EQ(read_handle.type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(read_handle.rights, reduced_rights);

  zx_handle_close(read_handle.handle);
}

TEST(ChannelWriteEtcTest, InvalidOperationFailure) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t event_raw = event.get();

  zx_handle_disposition_t disp = {
      .operation = 9999,  // Invalid
      .handle = event.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS,
      .result = ZX_OK,
  };

  ASSERT_EQ(local.write_etc(0, nullptr, 0, &disp, 1), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(disp.result, ZX_ERR_INVALID_ARGS);

  // The handle should have been closed anyway (since it was not DUPLICATE).
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(event_raw));
}

TEST(ChannelWriteEtcTest, WrongTypeFailure) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t event_raw = event.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event.release(),
      .type = ZX_OBJ_TYPE_VMO,  // Wrong type
      .rights = ZX_DEFAULT_EVENT_RIGHTS,
      .result = ZX_OK,
  };

  ASSERT_EQ(local.write_etc(0, nullptr, 0, &disp, 1), ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(disp.result, ZX_ERR_WRONG_TYPE);

  // Handle should be closed.
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(event_raw));
}

TEST(ChannelWriteEtcTest, MissingTransferRightFailure) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx::event event_dup;
  ASSERT_OK(event.duplicate(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_TRANSFER, &event_dup));
  zx_handle_t event_dup_raw = event_dup.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event_dup.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_TRANSFER,
      .result = ZX_OK,
  };

  ASSERT_EQ(local.write_etc(0, nullptr, 0, &disp, 1), ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(disp.result, ZX_ERR_ACCESS_DENIED);

  // Handle should be closed.
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(event_dup_raw));
}

TEST(ChannelWriteEtcTest, MissingDuplicateRightFailure) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx::event event_dup;
  ASSERT_OK(event.duplicate(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_DUPLICATE, &event_dup));
  zx_handle_t event_dup_raw = event_dup.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_DUPLICATE,
      .handle = event_dup.get(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_DUPLICATE,
      .result = ZX_OK,
  };

  ASSERT_EQ(local.write_etc(0, nullptr, 0, &disp, 1), ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(disp.result, ZX_ERR_ACCESS_DENIED);

  // Handle should STILL be valid (since it was DUPLICATE).
  EXPECT_OK(zx_handle_check_valid(event_dup_raw));
}

TEST(ChannelWriteEtcTest, ExpandedRightsFailure) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx::event event_dup;
  ASSERT_OK(event.duplicate(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_SIGNAL, &event_dup));
  zx_handle_t event_dup_raw = event_dup.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event_dup.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS,  // Requesting signal right which we don't have
      .result = ZX_OK,
  };

  ASSERT_EQ(local.write_etc(0, nullptr, 0, &disp, 1), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(disp.result, ZX_ERR_INVALID_ARGS);

  // Handle should be closed.
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(event_dup_raw));
}

TEST(ChannelWriteEtcTest, TransferSelfFailure) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx_handle_t local_raw = local.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = local_raw,
      .type = ZX_OBJ_TYPE_CHANNEL,
      .rights = ZX_DEFAULT_CHANNEL_RIGHTS,
      .result = ZX_OK,
  };

  zx_handle_t local_released = local.release();
  ASSERT_EQ(zx_channel_write_etc(local_released, 0, nullptr, 0, &disp, 1), ZX_ERR_NOT_SUPPORTED);
  EXPECT_EQ(disp.result, ZX_ERR_NOT_SUPPORTED);

  // The handle should have been closed.
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(local_released));
}

TEST(ChannelCallEtcTest, CallEtcSuccess) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  struct Request {
    zx_txid_t txid;
    char data[4];
  } request = {.txid = 0, .data = {'a', 'b', 'c', 'd'}};

  struct Reply {
    zx_txid_t txid;
    char data[4];
  } reply = {};

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx_handle_disposition_t wr_disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_DEFAULT_EVENT_RIGHTS,
      .result = ZX_OK,
  };

  zx_handle_info_t rd_info = {};

  zx_channel_call_etc_args_t args = {
      .wr_bytes = &request,
      .wr_handles = &wr_disp,
      .rd_bytes = &reply,
      .rd_handles = &rd_info,
      .wr_num_bytes = sizeof(request),
      .wr_num_handles = 1,
      .rd_num_bytes = sizeof(reply),
      .rd_num_handles = 1,
  };

  std::atomic<const char*> error = nullptr;

  std::jthread service_thread([&remote, &error]() {
    zx_status_t status = remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr);
    if (status != ZX_OK) {
      error = "remote wait failed";
      return;
    }

    struct Req {
      zx_txid_t txid;
      char data[4];
    } req = {};
    zx_handle_info_t req_handle = {};
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;

    status = remote.read_etc(0, &req, &req_handle, sizeof(req), 1, &actual_bytes, &actual_handles);
    if (status != ZX_OK) {
      error = "remote read failed";
      return;
    }

    if (actual_bytes != sizeof(req) || actual_handles != 1) {
      error = "unexpected read sizes";
      zx_handle_close(req_handle.handle);
      return;
    }

    if (req_handle.type != ZX_OBJ_TYPE_EVENT) {
      error = "unexpected handle type";
      zx_handle_close(req_handle.handle);
      return;
    }

    zx_handle_close(req_handle.handle);

    struct Rep {
      zx_txid_t txid;
      char data[4];
    } rep = {.txid = req.txid, .data = {'w', 'x', 'y', 'z'}};

    zx::event reply_event;
    status = zx::event::create(0, &reply_event);
    if (status != ZX_OK) {
      error = "failed to create reply event";
      return;
    }

    zx_handle_disposition_t rep_disp = {
        .operation = ZX_HANDLE_OP_MOVE,
        .handle = reply_event.release(),
        .type = ZX_OBJ_TYPE_EVENT,
        .rights = ZX_DEFAULT_EVENT_RIGHTS,
        .result = ZX_OK,
    };

    status = remote.write_etc(0, &rep, sizeof(rep), &rep_disp, 1);
    if (status != ZX_OK) {
      error = "remote write failed";
      return;
    }
  });

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(local.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  EXPECT_OK(wr_disp.result);

  EXPECT_EQ(actual_bytes, sizeof(reply));
  EXPECT_EQ(actual_handles, 1);
  EXPECT_EQ(reply.data[0], 'w');
  EXPECT_EQ(rd_info.type, ZX_OBJ_TYPE_EVENT);

  zx_handle_close(rd_info.handle);

  if (error != nullptr) {
    FAIL("Service Thread reported error: %s\n", error.load());
  }
}

TEST(ChannelCallEtcTest, CallEtcInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  char wr_buf[1] = {0};
  char rd_buf[1] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = wr_buf,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = 1,
      .wr_num_handles = 0,
      .rd_num_bytes = 1,
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(local.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, CreateInvalidOptionsReturnsInvalidArgs) {
  zx::channel local, remote;
  EXPECT_EQ(zx::channel::create(1, &local, &remote), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx::channel::create(0xFF, &local, &remote), ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, ReadInvalidOptionsReturnsNotSupported) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  EXPECT_EQ(remote.read(0xFF, nullptr, nullptr, 0, 0, nullptr, nullptr), ZX_ERR_NOT_SUPPORTED);
}

TEST(ChannelTest, ReadInvalidActualPointersReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  char byte = 'x';
  ASSERT_OK(local.write(0, &byte, sizeof(byte), nullptr, 0));

  uint32_t* bad_ptr = reinterpret_cast<uint32_t*>(1);
  char read_byte = 0;
  uint32_t actual_handles = 0;

  // Failing copy_to_user on actual_bytes.
  EXPECT_EQ(remote.read(0, &read_byte, nullptr, sizeof(read_byte), 0, bad_ptr, &actual_handles),
            ZX_ERR_INVALID_ARGS);

  // Re-write to ensure channel has a message for the next read attempt.
  ASSERT_OK(local.write(0, &byte, sizeof(byte), nullptr, 0));

  uint32_t actual_bytes = 0;
  // Failing copy_to_user on actual_handles.
  EXPECT_EQ(remote.read(0, &read_byte, nullptr, sizeof(read_byte), 0, &actual_bytes, bad_ptr),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, CallInvalidOptionsReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  char wr_buf[4] = {0};
  char rd_buf[4] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = wr_buf,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(wr_buf),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(local.call(0xFF, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, CallInvalidArgsPointerReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(local.call(0, zx::time::infinite(), reinterpret_cast<const zx_channel_call_args_t*>(1),
                       &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteIovecBoundedInvalidPointerReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Passing an invalid pointer with num_iovecs <= 16 (bounded).
  EXPECT_EQ(
      local.write(ZX_CHANNEL_WRITE_USE_IOVEC, reinterpret_cast<const void*>(1), 4, nullptr, 0),
      ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteIovecUnboundedInvalidPointerReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Passing an invalid pointer with num_iovecs > 16 (unbounded).
  EXPECT_EQ(
      local.write(ZX_CHANNEL_WRITE_USE_IOVEC, reinterpret_cast<const void*>(1), 20, nullptr, 0),
      ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteIovecBoundedNonZeroReservedReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  constexpr uint32_t kNumIovecs = 4;
  char data[kNumIovecs] = {0};
  zx_channel_iovec_t iovecs[kNumIovecs] = {};
  for (uint32_t i = 0; i < kNumIovecs; ++i) {
    iovecs[i].buffer = &data[i];
    iovecs[i].capacity = 1;
    iovecs[i].reserved = 0;
  }
  iovecs[2].reserved = 1;

  EXPECT_EQ(local.write(ZX_CHANNEL_WRITE_USE_IOVEC, iovecs, kNumIovecs, nullptr, 0),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteIovecUnboundedNonZeroReservedReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  constexpr uint32_t kNumIovecs = 20;
  char data[kNumIovecs] = {0};
  zx_channel_iovec_t iovecs[kNumIovecs] = {};
  for (uint32_t i = 0; i < kNumIovecs; ++i) {
    iovecs[i].buffer = &data[i];
    iovecs[i].capacity = 1;
    iovecs[i].reserved = 0;
  }
  // Set non-zero reserved field in the second chunk (index >= 16).
  iovecs[18].reserved = 1;

  EXPECT_EQ(local.write(ZX_CHANNEL_WRITE_USE_IOVEC, iovecs, kNumIovecs, nullptr, 0),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteIovecUnboundedSuccess) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  constexpr uint32_t kNumIovecs = 20;
  char data[kNumIovecs];
  zx_channel_iovec_t iovecs[kNumIovecs] = {};
  for (uint32_t i = 0; i < kNumIovecs; ++i) {
    data[i] = static_cast<char>('a' + i);
    iovecs[i].buffer = &data[i];
    iovecs[i].capacity = 1;
    iovecs[i].reserved = 0;
  }

  ASSERT_OK(local.write(ZX_CHANNEL_WRITE_USE_IOVEC, iovecs, kNumIovecs, nullptr, 0));

  char read_buf[kNumIovecs] = {0};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read(0, read_buf, nullptr, sizeof(read_buf), 0, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, kNumIovecs);
  EXPECT_EQ(actual_handles, 0u);
  for (uint32_t i = 0; i < kNumIovecs; ++i) {
    EXPECT_EQ(read_buf[i], static_cast<char>('a' + i));
  }
}

TEST(ChannelTest, WriteInvalidOptionsReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  char data = 'a';
  EXPECT_EQ(local.write(0xFF, &data, sizeof(data), nullptr, 0), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(local.write_etc(0xFF, &data, sizeof(data), nullptr, 0), ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, CallPayloadTooSmallReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Write payload size less than sizeof(zx_txid_t) (4 bytes).
  char wr_buf[2] = {0};
  char rd_buf[4] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = wr_buf,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(wr_buf),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);

  // 0-byte payload should also fail.
  args.wr_num_bytes = 0;
  EXPECT_EQ(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteIovecTooManyIovecsReturnsOutOfRange) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  EXPECT_EQ(
      local.write(ZX_CHANNEL_WRITE_USE_IOVEC, nullptr, ZX_CHANNEL_MAX_MSG_IOVECS + 1, nullptr, 0),
      ZX_ERR_OUT_OF_RANGE);
}

TEST(ChannelTest, WriteShortPayloadsSucceeds) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Test writing 0, 1, 2, and 3 byte payloads without a waiter.
  const uint8_t src_data[4] = {0x11, 0x22, 0x33, 0x44};

  for (uint32_t size = 0; size < 4; ++size) {
    ASSERT_OK(local.write(0, src_data, size, nullptr, 0));

    uint8_t dst_data[4] = {0};
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(
        remote.read(0, dst_data, nullptr, sizeof(dst_data), 0, &actual_bytes, &actual_handles));
    EXPECT_EQ(actual_bytes, size);
    EXPECT_EQ(actual_handles, 0u);
    if (size > 0) {
      EXPECT_BYTES_EQ(src_data, dst_data, size);
    }
  }

  // Now test with an active MessageWaiter to exercise get_txid() on short payloads.
  std::thread caller_thread([&local]() {
    zx_txid_t txid = 0;
    char rd_buf[8] = {0};
    zx_channel_call_args_t args = {
        .wr_bytes = &txid,
        .wr_handles = nullptr,
        .rd_bytes = rd_buf,
        .rd_handles = nullptr,
        .wr_num_bytes = sizeof(txid),
        .wr_num_handles = 0,
        .rd_num_bytes = sizeof(rd_buf),
        .rd_num_handles = 0,
    };
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    EXPECT_OK(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
    EXPECT_EQ(actual_bytes, sizeof(txid));
  });

  // Wait for the caller's request message to arrive on remote.
  ASSERT_OK(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));

  // Write short payloads to local while local has an active waiter.
  for (uint32_t size = 0; size < 4; ++size) {
    ASSERT_OK(remote.write(0, src_data, size, nullptr, 0));
  }

  // Read the caller's request and reply so the call completes deterministically.
  zx_txid_t call_txid = 0;
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(
      remote.read(0, &call_txid, nullptr, sizeof(call_txid), 0, &actual_bytes, &actual_handles));
  ASSERT_EQ(actual_bytes, sizeof(call_txid));
  ASSERT_OK(remote.write(0, &call_txid, sizeof(call_txid), nullptr, 0));

  caller_thread.join();

  // Verify that the short payloads queued on local can still be read.
  for (uint32_t size = 0; size < 4; ++size) {
    uint8_t dst_data[4] = {0};
    ASSERT_OK(
        local.read(0, dst_data, nullptr, sizeof(dst_data), 0, &actual_bytes, &actual_handles));
    EXPECT_EQ(actual_bytes, size);
    EXPECT_EQ(actual_handles, 0u);
    if (size > 0) {
      EXPECT_BYTES_EQ(src_data, dst_data, size);
    }
  }
}

TEST(ChannelTest, ReadBadBufferReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  const uint8_t data[8] = {1, 2, 3, 4, 5, 6, 7, 8};
  ASSERT_OK(remote.write(0, data, sizeof(data), nullptr, 0));

  void* bad_ptr = reinterpret_cast<void*>(1);
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_STATUS(local.read(0, bad_ptr, nullptr, sizeof(data), 0, &actual_bytes, &actual_handles),
                ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, CallBadReadBufferReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  std::thread server_thread([&remote]() {
    ASSERT_OK(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(remote.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_EQ(actual_bytes, sizeof(txid));

    struct Reply {
      zx_txid_t txid;
      uint32_t data;
    } reply = {.txid = txid, .data = 0x12345678};
    ASSERT_OK(remote.write(0, &reply, sizeof(reply), nullptr, 0));
  });

  zx_txid_t txid = 0;
  void* bad_ptr = reinterpret_cast<void*>(1);
  zx_channel_call_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = bad_ptr,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = 8,
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_STATUS(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
                ZX_ERR_INVALID_ARGS);

  server_thread.join();
}

TEST(ChannelTest, CallBadReadHandlesReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  std::thread server_thread([&remote]() {
    ASSERT_OK(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(remote.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_EQ(actual_bytes, sizeof(txid));

    zx::event reply_event;
    ASSERT_OK(zx::event::create(0, &reply_event));
    zx_handle_t handle = reply_event.release();

    struct Reply {
      zx_txid_t txid;
      uint32_t data;
    } reply = {.txid = txid, .data = 0x12345678};
    ASSERT_OK(remote.write(0, &reply, sizeof(reply), &handle, 1));
  });

  zx_txid_t txid = 0;
  uint8_t rd_bytes[8] = {0};
  zx_handle_t* bad_handles = reinterpret_cast<zx_handle_t*>(1);
  zx_channel_call_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rd_bytes,
      .rd_handles = bad_handles,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_bytes),
      .rd_num_handles = 1,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_STATUS(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
                ZX_ERR_INVALID_ARGS);

  server_thread.join();
}

TEST(ChannelTest, WriteEtcReadOnlyHandleDispositionReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  // Create and populate a VMO with an invalid handle disposition.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = ZX_HANDLE_INVALID,
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_RIGHT_SAME_RIGHTS,
      .result = ZX_OK,
  };
  ASSERT_OK(vmo.write(&disp, 0, sizeof(disp)));

  // Map the VMO as read-only.
  zx_vaddr_t vaddr = 0;
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0, zx_system_get_page_size(), &vaddr));

  auto* read_only_disp = reinterpret_cast<zx_handle_disposition_t*>(vaddr);
  const char data = 'x';
  EXPECT_STATUS(local.write_etc(0, &data, 1, read_only_disp, 1), ZX_ERR_INVALID_ARGS);

  ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, zx_system_get_page_size()));
}

TEST(ChannelTest, CallMismatchedTxidQueuesRegularMessage) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  struct MismatchedMsg {
    zx_txid_t txid;
    uint32_t payload;
  };

  std::thread server_thread([&remote]() {
    ASSERT_OK(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(remote.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_EQ(actual_bytes, sizeof(txid));

    // Send a message with a kernel-generated txid (>= 0x80000000) that does NOT match txid.
    // This exercises TryWriteToMessageWaiter searching waiters, finding no match, and falling
    // through.
    MismatchedMsg mismatched = {.txid = 0x8fffffff, .payload = 0xdeadbeef};
    ASSERT_OK(remote.write(0, &mismatched, sizeof(mismatched), nullptr, 0));

    // Now send the matching reply so local.call can complete.
    struct MatchingReply {
      zx_txid_t txid;
      uint32_t payload;
    } reply = {.txid = txid, .payload = 0xcafebabe};
    ASSERT_OK(remote.write(0, &reply, sizeof(reply), nullptr, 0));
  });

  zx_txid_t txid = 0;
  uint8_t rd_buf[8] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(local.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, sizeof(rd_buf));

  server_thread.join();

  // The mismatched message must be queued in local as a regular readable message.
  MismatchedMsg received_mismatched = {};
  ASSERT_OK(local.read(0, &received_mismatched, nullptr, sizeof(received_mismatched), 0,
                       &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, sizeof(received_mismatched));
  EXPECT_EQ(received_mismatched.txid, 0x8fffffff);
  EXPECT_EQ(received_mismatched.payload, 0xdeadbeef);
}

TEST(ChannelTest, ReadWithoutReadRightReturnsAccessDenied) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::channel reduced;
  ASSERT_OK(local.replace(ZX_DEFAULT_CHANNEL_RIGHTS & ~ZX_RIGHT_READ, &reduced));

  char buf[8] = {0};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_STATUS(reduced.read(0, buf, nullptr, sizeof(buf), 0, &actual_bytes, &actual_handles),
                ZX_ERR_ACCESS_DENIED);
}

struct SuspendedCallReply {
  zx_txid_t txid;
  uint32_t data;
};

void WaitForThreadState(const zx::thread& thread, zx_thread_state_t expected_state,
                        zx::duration timeout = zx::sec(5)) {
  const zx::time deadline = zx::deadline_after(timeout);
  while (zx::clock::get_monotonic() < deadline) {
    zx_info_thread_t info;
    ASSERT_OK(thread.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr));
    if (info.state == expected_state) {
      return;
    }
    zx::nanosleep(zx::deadline_after(zx::usec(100)));
  }
  FAIL("Thread did not reach expected state %u within timeout", expected_state);
}

void RunSuspendedCallTest(
    fit::function<void(const zx::channel&, zx_txid_t*, SuspendedCallReply*, uint32_t*, uint32_t*)>
        do_call,
    uint32_t expected_data) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::thread caller_thread_handle;
  std::jthread caller_thread([&]() {
    ASSERT_OK(zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &caller_thread_handle));

    zx_txid_t txid = 0;
    SuspendedCallReply reply_buf = {};
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    do_call(local, &txid, &reply_buf, &actual_bytes, &actual_handles);
    EXPECT_EQ(actual_bytes, sizeof(reply_buf));
    EXPECT_EQ(reply_buf.data, expected_data);
  });

  // Wait for the caller's request message to arrive on remote.
  // This wait also establishes a happens-before relationship ensuring that
  // caller_thread_handle has been populated by caller_thread before we read it.
  ASSERT_OK(remote.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));

  // Wait until the caller thread is blocked in channel call.
  ASSERT_NO_FATAL_FAILURE(
      WaitForThreadState(caller_thread_handle, ZX_THREAD_STATE_BLOCKED_CHANNEL));

  // Suspend the caller thread. This will cause the kernel waiter to return
  // ZX_ERR_INTERNAL_INTR_RETRY.
  zx::suspend_token token;
  ASSERT_OK(caller_thread_handle.suspend(&token));

  // Wait for the thread to enter the suspended state.
  ASSERT_OK(caller_thread_handle.wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));

  // Resume the caller thread by closing the suspend token.
  // The vDSO will call sys_channel_call_finish or sys_channel_call_etc_finish
  // to resume waiting.
  token.reset();
  ASSERT_OK(caller_thread_handle.wait_one(ZX_THREAD_RUNNING, zx::time::infinite(), nullptr));

  // Now the server reads the request and replies.
  zx_txid_t txid = 0;
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
  ASSERT_EQ(actual_bytes, sizeof(txid));

  SuspendedCallReply reply = {.txid = txid, .data = expected_data};
  ASSERT_OK(remote.write(0, &reply, sizeof(reply), nullptr, 0));
}

TEST(ChannelTest, CallSuspendedThreadResumesAndSucceeds) {
  constexpr uint32_t kExpectedData = 0x12345678;
  RunSuspendedCallTest(
      [](const zx::channel& local, zx_txid_t* txid, SuspendedCallReply* reply_buf,
         uint32_t* actual_bytes, uint32_t* actual_handles) {
        zx_channel_call_args_t args = {
            .wr_bytes = txid,
            .wr_handles = nullptr,
            .rd_bytes = reply_buf,
            .rd_handles = nullptr,
            .wr_num_bytes = sizeof(*txid),
            .wr_num_handles = 0,
            .rd_num_bytes = sizeof(*reply_buf),
            .rd_num_handles = 0,
        };
        EXPECT_OK(local.call(0, zx::time::infinite(), &args, actual_bytes, actual_handles));
      },
      kExpectedData);
}

TEST(ChannelTest, CallEtcSuspendedThreadResumesAndSucceeds) {
  constexpr uint32_t kExpectedData = 0x87654321;
  RunSuspendedCallTest(
      [](const zx::channel& local, zx_txid_t* txid, SuspendedCallReply* reply_buf,
         uint32_t* actual_bytes, uint32_t* actual_handles) {
        zx_channel_call_etc_args_t args = {
            .wr_bytes = txid,
            .wr_handles = nullptr,
            .rd_bytes = reply_buf,
            .rd_handles = nullptr,
            .wr_num_bytes = sizeof(*txid),
            .wr_num_handles = 0,
            .rd_num_bytes = sizeof(*reply_buf),
            .rd_num_handles = 0,
        };
        EXPECT_OK(local.call_etc(0, zx::time::infinite(), &args, actual_bytes, actual_handles));
      },
      kExpectedData);
}

TEST(ChannelTest, CreatePolicyDeniedReturnsAccessDenied) {
  if (getenv("NO_NEW_PROCESS")) {
    ZXTEST_SKIP("Running without the ZX_POL_NEW_PROCESS policy, skipping test case.");
  }
  zx::job child_job;
  ASSERT_OK(zx::job::create(*zx::job::default_job(), 0, &child_job));
  zx_policy_basic_v2_t policy = {
      .condition = ZX_POL_NEW_CHANNEL,
      .action = ZX_POL_ACTION_DENY,
      .flags = ZX_POL_OVERRIDE_ALLOW,
  };
  ASSERT_OK(child_job.set_policy(ZX_JOB_POL_RELATIVE, ZX_JOB_POL_BASIC_V2, &policy, 1));

  zx::process process;
  zx::vmar vmar;
  ASSERT_OK(zx::process::create(child_job, "test-proc", sizeof("test-proc"), 0u, &process, &vmar));
  zx::thread thread;
  ASSERT_OK(zx::thread::create(process, "test-thread", sizeof("test-thread"), 0u, &thread));

  zx::channel cmd_channel;
  ASSERT_OK(start_mini_process_etc(process.get(), thread.get(), vmar.get(), ZX_HANDLE_INVALID, true,
                                   cmd_channel.reset_and_get_address()));

  zx_handle_t transferred = ZX_HANDLE_INVALID;
  EXPECT_EQ(mini_process_cmd(cmd_channel.get(), MINIP_CMD_CREATE_CHANNEL, &transferred),
            ZX_ERR_ACCESS_DENIED);
  if (transferred != ZX_HANDLE_INVALID) {
    zx_handle_close(transferred);
  }
}

TEST(ChannelTest, ReadNullBytesWithNonZeroSizeReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  const char data = 'x';
  ASSERT_OK(local.write(0, &data, sizeof(data), nullptr, 0));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(remote.read(0, nullptr, nullptr, sizeof(data), 0, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, ReadNullHandlesWithNonZeroCountReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t h = event.release();
  ASSERT_OK(local.write(0, nullptr, 0, &h, 1));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(remote.read(0, nullptr, nullptr, 0, 1, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, ReadEtcNullHandlesWithNonZeroCountReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t h = event.release();
  ASSERT_OK(local.write(0, nullptr, 0, &h, 1));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(remote.read_etc(0, nullptr, nullptr, 0, 1, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, ReadEtcInvalidHandlesPointerReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t h = event.release();
  ASSERT_OK(local.write(0, nullptr, 0, &h, 1));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(remote.read_etc(0, nullptr, reinterpret_cast<zx_handle_info_t*>(1), 0, 1, &actual_bytes,
                            &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, ReadEtcReadOnlyHandlesPointerReturnsInvalidArgsAndDoesNotLeakHandles) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t h = event.release();
  ASSERT_OK(local.write(0, nullptr, 0, &h, 1));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  zx_vaddr_t vaddr = 0;
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0, zx_system_get_page_size(), &vaddr));
  auto* read_only_handles = reinterpret_cast<zx_handle_info_t*>(vaddr);

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(remote.read_etc(0, nullptr, read_only_handles, 0, 1, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);

  ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, zx_system_get_page_size()));
}

TEST(ChannelTest, ReadEtcMayDiscardDiscardsHandlesAndClosesThemOnBufferTooSmall) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t event_raw = event.get();
  zx_handle_t h = event.release();

  char data[4] = {'t', 'e', 's', 't'};
  ASSERT_OK(local.write(0, data, sizeof(data), &h, 1));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  // Read with 0 buffer size and MAY_DISCARD
  EXPECT_EQ(remote.read_etc(ZX_CHANNEL_READ_MAY_DISCARD, nullptr, nullptr, 0, 0, &actual_bytes,
                            &actual_handles),
            ZX_ERR_BUFFER_TOO_SMALL);
  EXPECT_EQ(actual_bytes, sizeof(data));
  EXPECT_EQ(actual_handles, 1u);

  // The discarded message is gone, channel is now empty
  EXPECT_EQ(remote.read_etc(0, nullptr, nullptr, 0, 0, &actual_bytes, &actual_handles),
            ZX_ERR_SHOULD_WAIT);

  // The event handle in the discarded message was closed and not in our table
  EXPECT_EQ(zx_handle_check_valid(event_raw), ZX_ERR_NOT_FOUND);
}

TEST(ChannelTest, ReadEtcPopulatesAllHandleInfoFieldsCorrectly) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx::event event_reduced;
  ASSERT_OK(event.duplicate(ZX_RIGHT_TRANSFER | ZX_RIGHT_SIGNAL, &event_reduced));

  zx::socket socket_s, socket_c;
  ASSERT_OK(zx::socket::create(ZX_SOCKET_STREAM, &socket_s, &socket_c));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_handle_disposition_t dispositions[3] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = event_reduced.release(),
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = socket_s.release(),
          .type = ZX_OBJ_TYPE_SOCKET,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = vmo.release(),
          .type = ZX_OBJ_TYPE_VMO,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };

  ASSERT_OK(local.write_etc(0, nullptr, 0, dispositions, 3));

  zx_handle_info_t handle_infos[3] = {};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read_etc(0, nullptr, handle_infos, 0, 3, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, 0u);
  EXPECT_EQ(actual_handles, 3u);

  EXPECT_NE(handle_infos[0].handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(handle_infos[0].type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(handle_infos[0].rights, ZX_RIGHT_TRANSFER | ZX_RIGHT_SIGNAL);
  EXPECT_EQ(handle_infos[0].unused, 0u);
  EXPECT_OK(zx_handle_close(handle_infos[0].handle));

  EXPECT_NE(handle_infos[1].handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(handle_infos[1].type, ZX_OBJ_TYPE_SOCKET);
  EXPECT_EQ(handle_infos[1].rights, ZX_DEFAULT_SOCKET_RIGHTS);
  EXPECT_EQ(handle_infos[1].unused, 0u);
  EXPECT_OK(zx_handle_close(handle_infos[1].handle));

  EXPECT_NE(handle_infos[2].handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(handle_infos[2].type, ZX_OBJ_TYPE_VMO);
  EXPECT_EQ(handle_infos[2].rights, ZX_DEFAULT_VMO_RIGHTS);
  EXPECT_EQ(handle_infos[2].unused, 0u);
  EXPECT_OK(zx_handle_close(handle_infos[2].handle));
}

TEST(ChannelTest, ReadNullActualPointersSucceeds) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  char write_byte = 'z';
  ASSERT_OK(local.write(0, &write_byte, sizeof(write_byte), nullptr, 0));

  char read_byte = 0;
  EXPECT_OK(remote.read(0, &read_byte, nullptr, sizeof(read_byte), 0, nullptr, nullptr));
  EXPECT_EQ(read_byte, 'z');
}

TEST(ChannelTest, ReadZeroByteZeroHandleMessageSucceeds) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  ASSERT_OK(local.write(0, nullptr, 0, nullptr, 0));

  uint32_t actual_bytes = 999;
  uint32_t actual_handles = 999;
  EXPECT_OK(remote.read(0, nullptr, nullptr, 0, 0, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, 0u);
  EXPECT_EQ(actual_handles, 0u);
}

TEST(ChannelTest, ReadFromWrongObjectTypeReturnsWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  char byte = 0;
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_read(event.get(), 0, &byte, nullptr, 1, 0, &actual_bytes, &actual_handles),
            ZX_ERR_WRONG_TYPE);
  zx_handle_info_t info = {};
  EXPECT_EQ(zx_channel_read_etc(event.get(), 0, &byte, &info, 1, 1, &actual_bytes, &actual_handles),
            ZX_ERR_WRONG_TYPE);
}

TEST(ChannelTest, WriteNullBytesWithNonZeroSizeReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  EXPECT_EQ(local.write(0, nullptr, 8, nullptr, 0), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(local.write_etc(0, nullptr, 8, nullptr, 0), ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteNullHandlesWithNonZeroCountReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  EXPECT_EQ(local.write(0, nullptr, 0, nullptr, 1), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(local.write_etc(0, nullptr, 0, nullptr, 1), ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteBadBytesPointerReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  EXPECT_EQ(local.write(0, reinterpret_cast<const void*>(1), 8, nullptr, 0), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(local.write_etc(0, reinterpret_cast<const void*>(1), 8, nullptr, 0),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteEtcBadHandlesPointerReturnsInvalidArgs) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  EXPECT_EQ(local.write_etc(0, nullptr, 0, reinterpret_cast<zx_handle_disposition_t*>(1), 1),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelTest, WriteEtcFirstErrorLatchedAndReturned) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event_ok, event_no_transfer;
  ASSERT_OK(zx::event::create(0, &event_ok));
  ASSERT_OK(zx::event::create(0, &event_no_transfer));
  zx::event event_no_trans_dup;
  ASSERT_OK(event_no_transfer.duplicate(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_TRANSFER,
                                        &event_no_trans_dup));

  zx_handle_disposition_t dispositions[3] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = event_ok.release(),
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = event_no_trans_dup.release(),
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = 0b100,  // Bad handle
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };

  EXPECT_EQ(local.write_etc(0, nullptr, 0, dispositions, 3), ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(dispositions[0].result, ZX_OK);
  EXPECT_EQ(dispositions[1].result, ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(dispositions[2].result, ZX_ERR_BAD_HANDLE);
}

TEST(ChannelTest, WriteEtcDuplicateSelfHandleReturnsNotSupported) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_DUPLICATE,
      .handle = local.get(),
      .type = ZX_OBJ_TYPE_CHANNEL,
      .rights = ZX_RIGHT_SAME_RIGHTS,
      .result = ZX_OK,
  };

  EXPECT_EQ(local.write_etc(0, nullptr, 0, &disp, 1), ZX_ERR_NOT_SUPPORTED);
  EXPECT_EQ(disp.result, ZX_ERR_NOT_SUPPORTED);
  // Source handle remains valid
  EXPECT_OK(zx_handle_check_valid(local.get()));
}

TEST(ChannelTest, WriteEtcMixedMoveAndDuplicateOnFailurePreservesDuplicateHandles) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::event event_dup, event_move;
  ASSERT_OK(zx::event::create(0, &event_dup));
  ASSERT_OK(zx::event::create(0, &event_move));

  zx_handle_t dup_raw = event_dup.get();
  zx_handle_t move_raw = event_move.get();

  zx_handle_disposition_t dispositions[3] = {
      {
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = dup_raw,
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = event_move.release(),
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = 0b100,  // Bad handle
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };

  EXPECT_EQ(local.write_etc(0, nullptr, 0, dispositions, 3), ZX_ERR_BAD_HANDLE);
  EXPECT_EQ(dispositions[0].result, ZX_OK);
  EXPECT_EQ(dispositions[1].result, ZX_OK);
  EXPECT_EQ(dispositions[2].result, ZX_ERR_BAD_HANDLE);

  // Duplicate handle should STILL be valid
  EXPECT_OK(zx_handle_check_valid(dup_raw));
  // Moved handle should have been closed/consumed
  EXPECT_EQ(zx_handle_check_valid(move_raw), ZX_ERR_NOT_FOUND);
}

TEST(ChannelTest, WriteToPeerClosedConsumesMovedHandles) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  remote.reset();  // Close peer

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t event_raw = event.get();
  zx_handle_t h = event.release();

  EXPECT_EQ(local.write(0, nullptr, 0, &h, 1), ZX_ERR_PEER_CLOSED);
  EXPECT_EQ(zx_handle_check_valid(event_raw), ZX_ERR_NOT_FOUND);
}

TEST(ChannelTest, WriteIovecTotalCapacityOverflowReturnsOutOfRange) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  char dummy = 0;
  zx_channel_iovec_t iovecs[2] = {
      {
          .buffer = &dummy,
          .capacity = 0x80000000,
          .reserved = 0,
      },
      {
          .buffer = &dummy,
          .capacity = 0x80000000,
          .reserved = 0,
      },
  };

  EXPECT_EQ(local.write(ZX_CHANNEL_WRITE_USE_IOVEC, iovecs, 2, nullptr, 0), ZX_ERR_OUT_OF_RANGE);
}

TEST(ChannelTest, WriteIovecZeroCapacityWithNullBufferSucceeds) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  char data1[] = "hello ";
  char data2[] = "world";
  zx_channel_iovec_t iovecs[3] = {
      {
          .buffer = data1,
          .capacity = sizeof(data1) - 1,
          .reserved = 0,
      },
      {
          .buffer = nullptr,
          .capacity = 0,
          .reserved = 0,
      },
      {
          .buffer = data2,
          .capacity = sizeof(data2) - 1,
          .reserved = 0,
      },
  };

  ASSERT_OK(local.write(ZX_CHANNEL_WRITE_USE_IOVEC, iovecs, 3, nullptr, 0));

  char read_buf[32] = {};
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(remote.read(0, read_buf, nullptr, sizeof(read_buf), 0, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, (sizeof(data1) - 1) + (sizeof(data2) - 1));
  EXPECT_BYTES_EQ(read_buf, "hello world", actual_bytes);
}

TEST(ChannelTest, CallWithoutReadRightReturnsAccessDenied) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::channel local_no_read;
  ASSERT_OK(local.replace(ZX_DEFAULT_CHANNEL_RIGHTS & ~ZX_RIGHT_READ, &local_no_read));

  char wr_buf[4] = {0};
  char rd_buf[4] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = wr_buf,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(wr_buf),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(local_no_read.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_ACCESS_DENIED);
}

TEST(ChannelTest, CallWithoutWriteRightReturnsAccessDenied) {
  zx::channel local, remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));

  zx::channel local_no_write;
  ASSERT_OK(local.replace(ZX_DEFAULT_CHANNEL_RIGHTS & ~ZX_RIGHT_WRITE, &local_no_write));

  char wr_buf[4] = {0};
  char rd_buf[4] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = wr_buf,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(wr_buf),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(local_no_write.call(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_ACCESS_DENIED);
}

TEST(ChannelTest, CallFromWrongObjectTypeReturnsWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  char wr_buf[4] = {0};
  char rd_buf[4] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = wr_buf,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(wr_buf),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_call(event.get(), 0, zx::time::infinite().get(), &args, &actual_bytes,
                            &actual_handles),
            ZX_ERR_WRONG_TYPE);
}

}  // namespace
}  // namespace channel
