// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDLCAT_LIB_ANALYTICS_H_
#define TOOLS_FIDLCAT_LIB_ANALYTICS_H_

#include "src/developer/debug/ipc/protocol.h"
#include "src/lib/analytics/cpp/core_dev_tools/analytics.h"

namespace fidlcat {

class Analytics : public analytics::core_dev_tools::Analytics<Analytics> {
 public:
 private:
  friend class analytics::core_dev_tools::Analytics<Analytics>;

  static constexpr char kToolName[] = "fidlcat";
  static constexpr uint32_t kToolVersion = debug_ipc::kCurrentProtocolVersion;
  static constexpr int64_t kQuitTimeoutMs = 500;
  static constexpr char kMeasurementId[] = "G-Q65G4CDNFV";
  static constexpr char kMeasurementKey[] = "laylnxMAQn6eMJW3vsaPww";
  static constexpr char kEnableArgs[] = "--analytics=enable";
  static constexpr char kDisableArgs[] = "--analytics=disable";
  static constexpr char kStatusArgs[] = "--analytics-show";
};

}  // namespace fidlcat

#endif  // TOOLS_FIDLCAT_LIB_ANALYTICS_H_
