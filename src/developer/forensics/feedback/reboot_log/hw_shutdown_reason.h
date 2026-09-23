// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_HW_SHUTDOWN_REASON_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_HW_SHUTDOWN_REASON_H_

#include <cstdint>

namespace forensics::feedback {

enum class HwShutdownReason : std::uint8_t {
  kNotSet,
  kUndefined,
  kCold,
  kWarm,
  kBrownout,
  kWatchdog,
  kUserHardReset,
  kNotParseable,
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_HW_SHUTDOWN_REASON_H_
