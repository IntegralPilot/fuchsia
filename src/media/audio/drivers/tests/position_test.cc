// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/tests/position_test.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>

#include <cstring>
#include <iomanip>
#include <sstream>

#include <gtest/gtest.h>

namespace media::audio::drivers::test {

// Start recording position/timestamps, set notifications to request another, and request the first
void PositionTest::EnablePositionNotifications() {
  record_position_info_ = true;
  request_next_position_notification_ = true;
  RequestPositionNotification();
}

void PositionTest::PositionNotificationCallback(
    fuchsia::hardware::audio::RingBufferPositionInfo position_info) {
  zx::time now = zx::clock::get_monotonic();
  zx::time position_time = zx::time(position_info.timestamp);

  AdminTest::PositionNotificationCallback(position_info);

  EXPECT_LT(start_time(), now);
  EXPECT_LT(position_time, now);

  if (position_notification_count_) {
    EXPECT_GT(position_time, start_time());
    EXPECT_GT(position_time, zx::time(saved_position_.timestamp));
  } else {
    EXPECT_GE(position_time, start_time());
  }
  EXPECT_LT(position_info.position, ring_buffer_frames() * frame_size());

  // If we want to continue to chain of position notifications, request the next one.
  if (request_next_position_notification_) {
    RequestPositionNotification();
  }

  // If we don't need to update our running stats on position, exit now.
  if (!record_position_info_) {
    return;
  }

  if constexpr (kLogDetailedPositionInfo) {
    notifications_.push_back({
        .position = position_info.position,
        .timestamp = position_info.timestamp,
        .arrival_time = now.get(),
    });
  }

  ++position_notification_count_;

  // The `.position` reported by a position notification is a byte position within the ring buffer.
  // For long-running byte position, we could maintain a `running_position_` (a uint64_t initialized
  // to 0 upon Start()) that is updated by the algorithm below. This uses `.position` as a ring
  // "modulo" and adds the buffer size when it detects rollover, so it does not account for "sparse"
  // position notifications that occur more than a ring-buffer apart. For this technique to be
  // accurate, the ring-buffer client must (1) set position notification frequency to 2/buffer or
  // greater and (2) register for notifications actively enough that the position advanced between
  // notifications never exceeds the ring-buffer size.
  //   running_position_ += position_info.position;
  //   running_position_ -= saved_position_.position;
  //   if (position_info.position <= saved_position_.position) {
  //     running_position_ += (ring_buffer_frames() * frame_size());
  //   }

  saved_position_.timestamp = position_info.timestamp;
  saved_position_.position = position_info.position;
}

// Wait for the specified number of position notifications, then stop recording timestamp data.
// ...but don't DisablePositionNotifications, in case later notifications surface other issues.
void PositionTest::ExpectPositionNotifyCount(uint32_t count) {
  RunLoopUntil([this, count]() { return position_notification_count_ >= count || HasFailure(); });

  record_position_info_ = false;
}

// What timestamp do we expect, for the final notification received? We know how many
// notifications we've received; we'll multiply this by the per-notification time duration.
void PositionTest::ValidatePositionInfo() {
  zx::duration notification_timestamp = zx::time(saved_position_.timestamp) - start_time();
  zx::duration arrived_timestamp = zx::clock::get_monotonic() - start_time();

  ASSERT_GT(position_notification_count_, 0u) << "No position notifications received";
  ASSERT_GT(ring_buffer_pcm_format().frame_rate, 0u) << "Frame rate cannot be zero";

  // nsec/notification = nsec/sec * sec/frames * frames/ring * ring/notification
  int64_t nsec_per_notif = zx::sec(1).to_nsecs() * ring_buffer_frames();
  nsec_per_notif /=
      static_cast<int64_t>(ring_buffer_pcm_format().frame_rate * notifications_per_ring());

  // Upon enabling notifications, our first notification might arrive immediately. Thus, the average
  // number of notification periods elapsed is (position_notification_count_ - 0.5).
  auto expected_timestamp =
      zx::duration((position_notification_count_ * nsec_per_notif) - (nsec_per_notif / 2));

  // Delivery-time requirements for pos notifications are loose; include a tolerance of +/-2 notifs.
  auto timestamp_tolerance = zx::duration(nsec_per_notif * 2);
  auto min_allowed_timestamp = expected_timestamp - timestamp_tolerance;
  auto max_allowed_timestamp = expected_timestamp + timestamp_tolerance;

  if (notification_timestamp < min_allowed_timestamp ||
      notification_timestamp > max_allowed_timestamp || arrived_timestamp < min_allowed_timestamp) {
    std::ostringstream timestamps;
    timestamps << "Expected [ min " << min_allowed_timestamp.to_nsecs() << ", ideal "
               << expected_timestamp.to_nsecs() << ", max " << max_allowed_timestamp.to_nsecs()
               << " ], actual " << notification_timestamp.to_nsecs() << " (arrived "
               << arrived_timestamp.to_nsecs() << ")";
    EXPECT_GT(notification_timestamp.to_nsecs(), min_allowed_timestamp.to_nsecs())
        << timestamps.str() << "- notifications occurring too rapidly.";
    EXPECT_LT(notification_timestamp.to_nsecs(), max_allowed_timestamp.to_nsecs())
        << timestamps.str() << " - notifications occurring too slowly.";

    // Also validate when the notification was actually received (not just the timestamp).
    EXPECT_GT(arrived_timestamp.to_nsecs(), min_allowed_timestamp.to_nsecs())
        << timestamps.str() << " - notification arrived too early.";

    if constexpr (kLogDetailedPositionInfo) {
      auto ring_buffer_bytes = ring_buffer_frames() * frame_size();
      FX_LOGS(INFO) << "Start time " << start_time().get() << ", RingBuffer "
                    << ring_buffer_frames() << " frames (" << ring_buffer_bytes << " bytes), "
                    << ring_buffer_pcm_format().frame_rate << " Hz, " << nsec_per_notif
                    << " nsec/notif, " << nsec_per_notif * notifications_per_ring()
                    << " nsec/ring.";
      FX_LOGS(INFO) << "    Notif    Position___Delta" << "           Timestamp_____Delta   "
                    << "             Arrival_____Delta";
      for (auto idx = 0u; idx < notifications_.size(); ++idx) {
        uint32_t position_delta = (ring_buffer_bytes + notifications_[idx].position -
                                   (idx == 0u ? 0 : notifications_[idx - 1].position)) %
                                  ring_buffer_bytes;
        int64_t timestamp_delta =
            notifications_[idx].timestamp -
            (idx == 0 ? start_time().get() : notifications_[idx - 1].timestamp);
        int64_t arrival_delta =
            notifications_[idx].arrival_time -
            (idx == 0 ? start_time().get() : notifications_[idx - 1].arrival_time);

        FX_LOGS(INFO) << "   [ " << std::setw(2) << idx << " ]"             //
                      << std::setw(12) << notifications_[idx].position      //
                      << std::setw(8) << position_delta                     //
                      << std::setw(21) << notifications_[idx].timestamp     //
                      << std::setw(12) << timestamp_delta                   //
                      << std::setw(21) << notifications_[idx].arrival_time  //
                      << std::setw(12) << arrival_delta;
      }
      FX_LOGS(INFO) << "Expected values:     " << std::setw(8)
                    << ring_buffer_bytes / notifications_per_ring() << "                     "
                    << std::setw(12) << nsec_per_notif << "                     " << std::setw(12)
                    << nsec_per_notif;
    }
  }
}

#define DEFINE_POSITION_TEST_CLASS(CLASS_NAME, CODE)                               \
  class CLASS_NAME : public PositionTest {                                         \
   public:                                                                         \
    explicit CLASS_NAME(const DeviceEntry& dev_entry) : PositionTest(dev_entry) {} \
    void TestBody() override { CODE }                                              \
  }

//
// Test cases that target various position notification behaviors.
//
// Any case not ending in disconnect/error should WaitForError, in case the channel disconnects.

// Verify position notifications at fast rate (64/sec: 32 notifs/ring in a 0.5-second buffer).
DEFINE_POSITION_TEST_CLASS(PositionNotifyFast, {
  constexpr auto kNotifsPerRingBuffer = 32u;
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  if (properties() && properties()->manufacturer == "QEMU" &&
      properties()->is_input.value_or(false)) {
    GTEST_SKIP()
        << "*** QEMU audio input claims 16k but actually runs at 12.8k. Skipping this test. ***";
  }
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  // Request a 0.5-second ring-buffer.
  ASSERT_NO_FAILURE_OR_SKIP(
      RequestBuffer(ring_buffer_pcm_format().frame_rate / 2, kNotifsPerRingBuffer));
  ASSERT_NO_FAILURE_OR_SKIP(EnablePositionNotifications());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStart());

  // After numerous notifications (in this case, twice around the ring), stop updating position info
  // (but let notifications continue). Ensure that the rate of advance is within acceptable range.
  ExpectPositionNotifyCount(kNotifsPerRingBuffer * 2);
  ValidatePositionInfo();

  WaitForError();
});

// Verify position notifications at slow rate (1/sec: 2 notifs/ring in a 2-second buffer).
DEFINE_POSITION_TEST_CLASS(PositionNotifySlow, {
  constexpr auto kNotifsPerRingBuffer = 2u;
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  // Request a 2-second ring-buffer.
  ASSERT_NO_FAILURE_OR_SKIP(
      RequestBuffer(ring_buffer_pcm_format().frame_rate * 2, kNotifsPerRingBuffer));
  ASSERT_NO_FAILURE_OR_SKIP(EnablePositionNotifications());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStart());

  // After numerous notifications (in this case, twice around the ring), stop updating position info
  // (but let notifications continue). Ensure that the rate of advance is within acceptable range.
  ExpectPositionNotifyCount(kNotifsPerRingBuffer * 2);
  ValidatePositionInfo();

  // Wait longer than the default (100 ms), as notifications are less frequent than that.
  zx::duration time_per_notif = zx::sec(1) * ring_buffer_frames() /
                                ring_buffer_pcm_format().frame_rate / kNotifsPerRingBuffer;
  WaitForError(time_per_notif);
});

// Verify that NO position notifications arrive after Stop is called.
DEFINE_POSITION_TEST_CLASS(NoMorePositionNotifyAfterStop, {
  constexpr auto kNotifsPerRingBuffer = 32u;
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  // Set notifications to be rapid, with a small ring buffer and a large notifications-per-buffer.
  // If the device supports 192 kHz and the driver supports a ring this small, the buffer will be
  // 32 ms and notifications should arrive every 1 msec!
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(6144, kNotifsPerRingBuffer));
  ASSERT_NO_FAILURE_OR_SKIP(EnablePositionNotifications());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStart());

  //  After just a few position notifications, stop the ring buffer. From the Stop callback itself,
  // register a position callback that will fail the test if any further notification occurs.
  ASSERT_NO_FAILURE_OR_SKIP(ExpectPositionNotifyCount(3u));
  RequestRingBufferStopAndExpectNoPositionNotifications();
  WaitForError();
});

// Register separate test case instances for each enumerated device
//
// See googletest/docs/advanced.md for details
#define REGISTER_POSITION_TEST(CLASS_NAME, DEVICE)                                              \
  testing::RegisterTest("PositionTest", TestNameForEntry(#CLASS_NAME, DEVICE).c_str(), nullptr, \
                        DevNameForEntry(DEVICE).c_str(), __FILE__, __LINE__,                    \
                        [&]() -> PositionTest* { return new CLASS_NAME(DEVICE); })

#define REGISTER_DISABLED_POSITION_TEST(CLASS_NAME, DEVICE)                                       \
  testing::RegisterTest(                                                                          \
      "PositionTest", (std::string("DISABLED_") + TestNameForEntry(#CLASS_NAME, DEVICE)).c_str(), \
      nullptr, DevNameForEntry(DEVICE).c_str(), __FILE__, __LINE__,                               \
      [&]() -> PositionTest* { return new CLASS_NAME(DEVICE); })

void RegisterPositionTestsForDevice(const DeviceEntry& device_entry) {
  // Codec drivers have no RingBuffers, and thus require no position tests.
  if (device_entry.isCodec()) {
    return;
  }

  REGISTER_POSITION_TEST(PositionNotifySlow, device_entry);
  REGISTER_POSITION_TEST(PositionNotifyFast, device_entry);
  REGISTER_POSITION_TEST(NoMorePositionNotifyAfterStop, device_entry);
}

}  // namespace media::audio::drivers::test
