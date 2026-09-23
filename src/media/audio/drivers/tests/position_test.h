// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_TESTS_POSITION_TEST_H_
#define SRC_MEDIA_AUDIO_DRIVERS_TESTS_POSITION_TEST_H_

#include "src/media/audio/drivers/tests/admin_test.h"

namespace media::audio::drivers::test {

// Position cases are default-disabled; if they DO run and fail, display verbose notification info.
inline constexpr bool kLogDetailedPositionInfo = true;

// This child of the AdminTest class is intended to run only in real-time capable environments.
class PositionTest : public AdminTest {
 public:
  explicit PositionTest(const DeviceEntry& dev_entry) : AdminTest(dev_entry) {}

 protected:
  // Request a position notification that will record timestamp/position and register for another.
  void EnablePositionNotifications();
  // Clear flag so that any pending position notification will not request yet another.
  void DisablePositionNotifications() { request_next_position_notification_ = false; }

  void PositionNotificationCallback(
      fuchsia::hardware::audio::RingBufferPositionInfo position_info) override;
  void ExpectPositionNotifyCount(uint32_t count);
  void ValidatePositionInfo();

 private:
  fuchsia::hardware::audio::RingBufferPositionInfo saved_position_ = {};

  // Watching for position info is a hanging-get. On receipt, this flag determines whether we
  // register for the next notification.
  bool request_next_position_notification_ = false;
  bool record_position_info_ = false;
  uint32_t position_notification_count_ = 0;

  // Only used when kLogDetailedPositionInfo is set
  struct NotificationData {
    uint32_t position;
    int64_t timestamp;
    int64_t arrival_time;
  };
  std::vector<NotificationData> notifications_;
};

}  // namespace media::audio::drivers::test

#endif  // SRC_MEDIA_AUDIO_DRIVERS_TESTS_POSITION_TEST_H_
