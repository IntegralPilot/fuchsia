// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_SYSTEM_INFO_H_
#define SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_SYSTEM_INFO_H_

#include <string>

namespace analytics {

// DEPRECATED: will be removed when UA support is stopped
// Get the same output as `uname -ms`
std::string GetOsVersion();

struct SystemInfo {
  std::string os;
  std::string arch;
};

SystemInfo GetSystemInfo();

}  // namespace analytics

#endif  // SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_SYSTEM_INFO_H_
