// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTS_INTEGRATION_INPUT_TESTS_STARNIX_INPUT_RELAY_API_H_
#define SRC_UI_TESTS_INTEGRATION_INPUT_TESTS_STARNIX_INPUT_RELAY_API_H_

#include <cstddef>
#include <cstdint>
#include <string>

namespace relay_api {

constexpr std::string kWaitForStdinMessage = "WAIT_FOR_STDIN";
constexpr std::string kReadyMessage = "READY";
constexpr std::string kFailedMessage = "FAILED";
constexpr std::string kEventDelimiter = "EVENT";
constexpr char kEventFormat[] = "EVENT tv_sec=%ld tv_usec=%ld type=%hu code=%hu value=%d";
constexpr std::string kQuitCmd = "quit";
constexpr std::string kEventCmd = "watch_event";
constexpr std::string kDeviceDelimiter = "SET_DEVICE";
constexpr std::string kTouchDev = "touch";
constexpr std::string kMouseDev = "mouse";

// The formatted event string will be sent across systems, so verify that the
// size of a `long` is the same on both sides. Similarly for `unsigned short`.
static_assert(sizeof(long) == sizeof(int64_t));
static_assert(sizeof(unsigned short) == sizeof(uint16_t));

namespace internal {
constexpr size_t kMaxDigitsPerLong = 20;          // -9,223,372,036,854,775,808
constexpr size_t kMaxDigitsPerUnsignedShort = 5;  // 65,535
}  // namespace internal

constexpr size_t kMaxPacketLen = sizeof(kEventFormat) + 3 * internal::kMaxDigitsPerLong +
                                 2 * internal::kMaxDigitsPerUnsignedShort;

// Touch down is expressed in six `uapi::input_event`s: BTN_TOUCH, ABS_MT_SLOT,
// ABS_MT_TRACKING_ID, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, and EV_SYN.
constexpr size_t kDownNumPackets = 6;

// Touch move expressed in four: ABS_MT_SLOT, ABS_MT_POSITION_X,
// ABS_MT_POSITION_Y, and EV_SYN.
constexpr size_t kMoveNumPackets = 4;

// Touch up is expressed in four: BTN_TOUCH, ABS_MT_SLOT, ABS_MT_TRACKING_ID,
// and EV_SYN.
constexpr size_t kUpNumPackets = 4;

constexpr size_t kDownUpNumPackets = kDownNumPackets + kUpNumPackets;

// Mouse wheel scroll is expressed in two `uapi::input_event`s: REL_WHEEL and EV_SYN.
constexpr size_t kScrollNumPackets = 2;

}  // namespace relay_api

#endif  // SRC_UI_TESTS_INTEGRATION_INPUT_TESTS_STARNIX_INPUT_RELAY_API_H_
