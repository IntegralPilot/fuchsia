// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_EXCEPTIONS_CONSTANTS_H_
#define SRC_DEVELOPER_FORENSICS_EXCEPTIONS_CONSTANTS_H_

#include <lib/zx/time.h>

namespace forensics {
namespace exceptions {

constexpr size_t kMaxNumExceptionHandlers = 10;
constexpr zx::duration kExceptionTtl = zx::min(5);

// The handler waits on a response from the component lookup service for 30 seconds.
//
// 30 seconds is not the upper bound on how long exception handling takes in total because minidump
// generation happens prior to component lookup and could take some arbitrary time.
constexpr zx::duration kComponentLookupTimeout{zx::sec(30)};

// The handler waits on a response from the feedback service for 15 seconds. This value is intended
// to allow for most, but not all requests to return a response before the timeout is hit.
constexpr zx::duration kFileReportTimeout{zx::sec(15)};

constexpr const char kCrashProcessStateKey[] = "crash.process.state";

constexpr const char kSuspendEnabledFlag[] = "--suspend_enabled";
constexpr const char kNoSuspendEnabledFlag[] = "--nosuspend_enabled";

// The exceptions power element is not active.
// System suspension is allowed.
constexpr uint8_t kPowerLevelInactive = 0u;

// The exceptions power element is active.
// System suspension is not allowed.
constexpr uint8_t kPowerLevelActive = 1u;

constexpr zx::duration kWakeLeaseAcquisitionTimeout{zx::sec(5)};

}  // namespace exceptions
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_EXCEPTIONS_CONSTANTS_H_
