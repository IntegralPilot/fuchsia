// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/epoll.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>

#include <cstring>
#include <string>

#include <gtest/gtest.h>
#include <linux/input-event-codes.h>
#include <linux/input.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

const uint32_t kTouchInputMinor = 0;
const uint32_t kKeyboardInputMinor = 1;
const uint32_t kMouseInputMinor = 2;

constexpr size_t min_bytes(size_t n_bits) { return (n_bits + 7) / 8; }

template <size_t SIZE>
bool get_bit(const std::array<uint8_t, SIZE>& buf, size_t bit_num) {
  size_t byte_index = bit_num / 8;
  size_t bit_index = bit_num % 8;
  EXPECT_LT(byte_index, SIZE) << "get_bit(" << bit_num << ") with array of only " << SIZE
                              << " elements";
  return buf[byte_index] & (1 << bit_index);
}

// TODO(quiche): Maybe move this to a test fixture, and guarantee removal of the input
// node between test cases.
fbl::unique_fd GetInputFile(const uint32_t kInputMinor) {
  // TODO(b/310963779): Here should directly /dev/input/eventX.

  // Typically, this would be `/dev/input/event0` or `/dev/input/event1`, but there's
  // not much to be gained by exercising `mkdir()` in these tests.
  std::string kInputFile = "/dev/input" + std::to_string(kInputMinor);

  // Create device node. Allow `EEXIST`, to avoid requiring each test case to remove the
  // input device node.
  const uint32_t kInputMajor = 13;
  if (mknod(kInputFile.c_str(), 0600 | S_IFCHR, makedev(kInputMajor, kInputMinor)) != 0 &&
      errno != EEXIST) {
    ADD_FAILURE() << " creating " << kInputFile << " failed: " << strerror(errno);
  };

  // Open device node.
  fbl::unique_fd fd(open(kInputFile.c_str(), O_RDONLY));
  EXPECT_TRUE(fd.is_valid()) << " failed to open " << kInputFile << ": " << strerror(errno);

  return fd;
}

TEST(InputTest, DevicePropertiesMatchTouchProperties) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  auto fd = GetInputFile(kTouchInputMinor);
  ASSERT_TRUE(fd.is_valid());

  // Getting the driver version must succeed, but the actual value doesn't matter.
  {
    uint32_t buf;
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGVERSION, &buf)) << "get version failed: " << strerror(errno);
  }

  // Getting the device identifier must succeed, but the actual value doesn't matter.
  {
    input_id buf;
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGID, &buf)) << "get identifier failed: " << strerror(errno);
  }

  // Getting the supported keys must succeed, with `BTN_TOUCH` supported.
  {
    constexpr auto kBufSize = min_bytes(KEY_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_KEY, kBufSize), &buf))
        << "get supported keys failed: " << strerror(errno);
    ASSERT_TRUE(get_bit(buf, BTN_TOUCH)) << " BTN_TOUCH not supported (but should be)";
    ASSERT_FALSE(get_bit(buf, BTN_TOOL_FINGER)) << " BTN_TOOL_FINGER should not be supported";
  }

  // Getting the supported absolute position attributes must succeed, with `ABS_X` and
  // `ABS_Y` not supported, and ABS_MT_ supported.
  {
    constexpr auto kBufSize = min_bytes(ABS_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_ABS, kBufSize), &buf))
        << "get supported absolute position failed: " << strerror(errno);
    ASSERT_FALSE(get_bit(buf, ABS_X)) << " ABS_X should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_Y)) << " ABS_Y should not be supported";
    ASSERT_TRUE(get_bit(buf, ABS_MT_SLOT)) << " ABS_MT_SLOT should not be supported";
    ASSERT_TRUE(get_bit(buf, ABS_MT_TRACKING_ID))
        << " ABS_MT_TRACKING_ID not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, ABS_MT_POSITION_X))
        << " ABS_MT_POSITION_X not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, ABS_MT_POSITION_Y))
        << " ABS_MT_POSITION_Y not supported (but should be)";
  }

  // Getting the supported relative motive attributes must succeed, but the actual values
  // don't matter.
  {
    constexpr auto kBufSize = min_bytes(REL_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_REL, kBufSize), &buf))
        << "get supported relative motion failed: " << strerror(errno);
  }

  // Getting the supported switches must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(SW_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_SW, kBufSize), &buf))
        << "get supported switches failed: " << strerror(errno);
  }

  // Getting the supported LEDs must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(LED_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_LED, kBufSize), &buf))
        << "get supported LEDs failed: " << strerror(errno);
  }

  // Getting the supported force feedbacks must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(FF_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_FF, kBufSize), &buf))
        << "get supported force feedbacks failed: " << strerror(errno);
  }

  // Getting the supported miscellaneous features must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(MSC_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_MSC, kBufSize), &buf))
        << "get supported miscellaneous features failed: " << strerror(errno);
  }

  // Getting the input properties must succeed, with `INPUT_PROP_DIRECT` set.
  {
    constexpr auto kBufSize = min_bytes(INPUT_PROP_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGPROP(kBufSize), &buf))
        << "get supported input properties features failed: " << strerror(errno);
    ASSERT_TRUE(get_bit(buf, INPUT_PROP_DIRECT))
        << " INPUT_PROP_DIRECT not supported (but should be)";
  }

  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_SLOT), &buf))
        << "get slot info failed: " << strerror(errno);
    ASSERT_EQ(0.0, buf.minimum);
    ASSERT_EQ(buf.maximum, 10.0);
  }

  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_TRACKING_ID), &buf))
        << "get tracking id info failed: " << strerror(errno);
    ASSERT_EQ(0.0, buf.minimum);
    ASSERT_EQ(buf.maximum, 0x7FFFFFFF);
  }

  // Getting the x-axis range must succeed. The exact axis parameters are device dependent,
  // but some basic validation is possible.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_POSITION_X), &buf))
        << "get x-axis info failed: " << strerror(errno);
    ASSERT_EQ(0.0, buf.minimum);
    ASSERT_GT(buf.maximum, 0.0);
  }

  // Getting the y-axis range must succeed. The exact axis parameters are device dependent,
  // but some basic validation is possible.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_POSITION_Y), &buf))
        << "get y-axis info failed: " << strerror(errno);
    ASSERT_EQ(0.0, buf.minimum);
    ASSERT_GT(buf.maximum, 0.0);
  }
}

TEST(InputTest, DevicePropertiesMatchKeyboardProperties) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  auto fd = GetInputFile(kKeyboardInputMinor);
  ASSERT_TRUE(fd.is_valid());

  // Getting the driver version must succeed, but the actual value doesn't matter.
  {
    uint32_t buf;
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGVERSION, &buf)) << "get version failed: " << strerror(errno);
  }

  // Getting the device identifier must succeed, but the actual value doesn't matter.
  {
    input_id buf;
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGID, &buf)) << "get identifier failed: " << strerror(errno);
  }

  // Getting the supported keys must succeed, with `BTN_MISC` and `KEY_POWER` supported.
  {
    constexpr auto kBufSize = min_bytes(KEY_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_KEY, kBufSize), &buf))
        << "get supported keys failed: " << strerror(errno);
    ASSERT_TRUE(get_bit(buf, BTN_MISC)) << " BTN_MISC not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, KEY_POWER)) << " KEY_POWER not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, KEY_A)) << " KEY_A not supported (but should be)";
  }

  // Getting the supported absolute position attributes must succeed, but Keyboard should
  // not support touch attributes.
  {
    constexpr auto kBufSize = min_bytes(ABS_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_ABS, kBufSize), &buf))
        << "get supported absolute position failed: " << strerror(errno);
    ASSERT_FALSE(get_bit(buf, ABS_X)) << " ABS_X should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_Y)) << " ABS_Y should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_SLOT)) << " ABS_MT_SLOT should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_TRACKING_ID)) << " ABS_MT_TRACKING_ID should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_POSITION_X)) << " ABS_MT_POSITION_X should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_POSITION_Y)) << " ABS_MT_POSITION_Y should not be supported";
  }

  // Getting the supported relative motive attributes must succeed, but the actual values
  // don't matter.
  {
    constexpr auto kBufSize = min_bytes(REL_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_REL, kBufSize), &buf))
        << "get supported relative motion failed: " << strerror(errno);
  }

  // Getting the supported switches must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(SW_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_SW, kBufSize), &buf))
        << "get supported switches failed: " << strerror(errno);
  }

  // Getting the supported LEDs must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(LED_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_LED, kBufSize), &buf))
        << "get supported LEDs failed: " << strerror(errno);
  }

  // Getting the supported force feedbacks must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(FF_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_FF, kBufSize), &buf))
        << "get supported force feedbacks failed: " << strerror(errno);
  }

  // Getting the supported miscellaneous features must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(MSC_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_MSC, kBufSize), &buf))
        << "get supported miscellaneous features failed: " << strerror(errno);
  }

  // Getting the input properties must succeed, with `INPUT_PROP_DIRECT` set.
  {
    constexpr auto kBufSize = min_bytes(INPUT_PROP_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGPROP(kBufSize), &buf))
        << "get supported input properties features failed: " << strerror(errno);
    ASSERT_TRUE(get_bit(buf, INPUT_PROP_DIRECT))
        << " INPUT_PROP_DIRECT not supported (but should be)";
  }

  // Getting the ABS_MT_SLOT range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_SLOT), &buf))
        << "get slot info failed: " << strerror(errno);
  }

  // Getting the ABS_MT_TRACKING_ID range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_TRACKING_ID), &buf))
        << "get tracking id info failed: " << strerror(errno);
  }

  // Getting the x-axis range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_POSITION_X), &buf))
        << "get x-axis info failed: " << strerror(errno);
  }

  // Getting the y-axis range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_POSITION_Y), &buf))
        << "get y-axis info failed: " << strerror(errno);
  }
}

TEST(InputTest, DevicePropertiesMatchMouseProperties) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  auto fd = GetInputFile(kMouseInputMinor);
  ASSERT_TRUE(fd.is_valid());

  // Getting the driver version must succeed, but the actual value doesn't matter.
  {
    uint32_t buf;
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGVERSION, &buf)) << "get version failed: " << strerror(errno);
  }

  // Getting the device identifier must succeed, but the actual value doesn't matter.
  {
    input_id buf;
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGID, &buf)) << "get identifier failed: " << strerror(errno);
  }

  // Getting the supported keys must succeed, with BTN_LEFT (BTN_MOUSE), BTN_RIGHT, BTN_MIDDLE,
  // BTN_SIDE, and BTN_EXTRA supported. BTN_TOUCH must not be supported.
  {
    constexpr auto kBufSize = min_bytes(KEY_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_KEY, kBufSize), &buf))
        << "get supported keys failed: " << strerror(errno);
    // BTN_LEFT and BTN_MOUSE are aliases (0x110) in Linux UAPI.
    ASSERT_TRUE(get_bit(buf, BTN_MOUSE)) << " BTN_MOUSE should be supported";
    ASSERT_TRUE(get_bit(buf, BTN_LEFT)) << " BTN_LEFT should be supported";
    ASSERT_TRUE(get_bit(buf, BTN_RIGHT)) << " BTN_RIGHT should be supported";
    ASSERT_TRUE(get_bit(buf, BTN_MIDDLE)) << " BTN_MIDDLE should be supported";
    ASSERT_TRUE(get_bit(buf, BTN_SIDE)) << " BTN_SIDE should be supported";
    ASSERT_TRUE(get_bit(buf, BTN_EXTRA)) << " BTN_EXTRA should be supported";
    ASSERT_FALSE(get_bit(buf, BTN_TOUCH)) << " BTN_TOUCH should not be supported";
  }

  // Getting the supported absolute position attributes must succeed, but Mouse should
  // not support touch attributes.
  {
    constexpr auto kBufSize = min_bytes(ABS_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_ABS, kBufSize), &buf))
        << "get supported absolute position failed: " << strerror(errno);
    ASSERT_FALSE(get_bit(buf, ABS_X)) << " ABS_X should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_Y)) << " ABS_Y should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_SLOT)) << " ABS_MT_SLOT should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_TRACKING_ID)) << " ABS_MT_TRACKING_ID should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_POSITION_X)) << " ABS_MT_POSITION_X should not be supported";
    ASSERT_FALSE(get_bit(buf, ABS_MT_POSITION_Y)) << " ABS_MT_POSITION_Y should not be supported";
  }

  // Getting the supported relative motion attributes must succeed, with REL_X, REL_Y,
  // REL_WHEEL, and REL_HWHEEL supported.
  {
    constexpr auto kBufSize = min_bytes(REL_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_REL, kBufSize), &buf))
        << "get supported relative motion failed: " << strerror(errno);
    ASSERT_TRUE(get_bit(buf, REL_X)) << " REL_X not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, REL_Y)) << " REL_Y not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, REL_WHEEL)) << " REL_WHEEL not supported (but should be)";
    ASSERT_TRUE(get_bit(buf, REL_HWHEEL)) << " REL_HWHEEL not supported (but should be)";
  }

  // Getting the supported switches must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(SW_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_SW, kBufSize), &buf))
        << "get supported switches failed: " << strerror(errno);
  }

  // Getting the supported LEDs must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(LED_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_LED, kBufSize), &buf))
        << "get supported LEDs failed: " << strerror(errno);
  }

  // Getting the supported force feedbacks must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(FF_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_FF, kBufSize), &buf))
        << "get supported force feedbacks failed: " << strerror(errno);
  }

  // Getting the supported miscellaneous features must succeed, but the actual values don't matter.
  {
    constexpr auto kBufSize = min_bytes(MSC_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGBIT(EV_MSC, kBufSize), &buf))
        << "get supported miscellaneous features failed: " << strerror(errno);
  }

  // Getting the input properties must succeed, with `INPUT_PROP_POINTER` supported and
  // `INPUT_PROP_DIRECT` unsupported.
  {
    constexpr auto kBufSize = min_bytes(INPUT_PROP_MAX);
    std::array<uint8_t, kBufSize> buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGPROP(kBufSize), &buf))
        << "get supported input properties features failed: " << strerror(errno);
    ASSERT_FALSE(get_bit(buf, INPUT_PROP_DIRECT)) << " INPUT_PROP_DIRECT should not be supported";
    ASSERT_TRUE(get_bit(buf, INPUT_PROP_POINTER))
        << " INPUT_PROP_POINTER not supported (but should be)";
  }

  // Getting the ABS_MT_SLOT range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_SLOT), &buf))
        << "get slot info failed: " << strerror(errno);
  }

  // Getting the ABS_MT_TRACKING_ID range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_TRACKING_ID), &buf))
        << "get tracking id info failed: " << strerror(errno);
  }

  // Getting the x-axis range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_POSITION_X), &buf))
        << "get x-axis info failed: " << strerror(errno);
  }

  // Getting the y-axis range must succeed, but the actual values don't matter.
  {
    input_absinfo buf{};
    ASSERT_EQ(0, ioctl(fd.get(), EVIOCGABS(ABS_MT_POSITION_Y), &buf))
        << "get y-axis info failed: " << strerror(errno);
  }
}

TEST(InputTest, DeviceCanBeRegisteredWithEpoll) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  auto input_fd = GetInputFile(kTouchInputMinor);
  ASSERT_TRUE(input_fd.is_valid());

  fbl::unique_fd epoll_fd(epoll_create(1));  // Per `man` page, must be >0.
  ASSERT_TRUE(epoll_fd.is_valid()) << "failed to create epoll fd: " << strerror(errno);

  epoll_event epoll_params = {.events = EPOLLIN | EPOLLWAKEUP, .data = {.fd = input_fd.get()}};
  ASSERT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, input_fd.get(), &epoll_params))
      << " epoll_ctl() failed: " << strerror(errno);

  epoll_event event_buf[1];
  ASSERT_EQ(0, epoll_wait(epoll_fd.get(), event_buf, 1, 0));
}

TEST(InputTest, GetDeviceName) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  auto fd = GetInputFile(kTouchInputMinor);
  ASSERT_TRUE(fd.is_valid());

  // Getting the device name with zero buffer length must succeed and return 0 bytes without
  // underflowing.
  ASSERT_EQ(0, ioctl(fd.get(), EVIOCGNAME(0), nullptr))
      << "get name with 0 length failed: " << strerror(errno);

  // Getting the device name with non-zero buffer must succeed and return the name.
  char buf[256] = {};
  int ret = ioctl(fd.get(), EVIOCGNAME(sizeof(buf)), buf);
  ASSERT_GT(ret, 0) << "get name failed: " << strerror(errno);
  EXPECT_GT(strlen(buf), 0u);
}

// The evdev current-state queries report what the device is doing right now (which keys
// are held, which LEDs are lit, which sounds are playing, which switches are toggled), as
// opposed to the EVIOCGBIT queries above which report what the device is capable of.
//
// Starnix does not track this state and reports all-zeros, but these must not fail.
// `xf86-input-evdev` issues all four unconditionally during PreInit and treats any error
// as fatal, so returning EINVAL here stops X11 from bringing up any input device at all.
TEST(InputTest, CurrentStateQueriesSucceed) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  for (const uint32_t minor : {kTouchInputMinor, kKeyboardInputMinor, kMouseInputMinor}) {
    auto fd = GetInputFile(minor);
    ASSERT_TRUE(fd.is_valid());

    {
      std::array<uint8_t, min_bytes(KEY_MAX)> buf;
      buf.fill(0xAA);
      ASSERT_EQ(0, ioctl(fd.get(), EVIOCGKEY(buf.size()), buf.data()))
          << "minor " << minor << ": get key state failed: " << strerror(errno);
      for (size_t i = 0; i < buf.size(); i++) {
        EXPECT_EQ(0, buf[i]) << "minor " << minor << ": no key should be reported as held";
      }
    }

    {
      std::array<uint8_t, min_bytes(LED_MAX)> buf;
      buf.fill(0xAA);
      ASSERT_EQ(0, ioctl(fd.get(), EVIOCGLED(buf.size()), buf.data()))
          << "minor " << minor << ": get led state failed: " << strerror(errno);
      for (size_t i = 0; i < buf.size(); i++) {
        EXPECT_EQ(0, buf[i]) << "minor " << minor << ": no LED should be reported as lit";
      }
    }

    {
      std::array<uint8_t, min_bytes(SND_MAX)> buf;
      buf.fill(0xAA);
      ASSERT_EQ(0, ioctl(fd.get(), EVIOCGSND(buf.size()), buf.data()))
          << "minor " << minor << ": get sound state failed: " << strerror(errno);
      for (size_t i = 0; i < buf.size(); i++) {
        EXPECT_EQ(0, buf[i]) << "minor " << minor << ": no sound should be reported as playing";
      }
    }

    {
      std::array<uint8_t, min_bytes(SW_MAX)> buf;
      buf.fill(0xAA);
      ASSERT_EQ(0, ioctl(fd.get(), EVIOCGSW(buf.size()), buf.data()))
          << "minor " << minor << ": get switch state failed: " << strerror(errno);
      for (size_t i = 0; i < buf.size(); i++) {
        EXPECT_EQ(0, buf[i]) << "minor " << minor << ": no switch should be reported as toggled";
      }
    }
  }
}

}  // namespace
