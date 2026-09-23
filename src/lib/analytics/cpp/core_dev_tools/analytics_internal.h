// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_ANALYTICS_INTERNAL_H_
#define SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_ANALYTICS_INTERNAL_H_

#include <string_view>

#include "src/lib/analytics/cpp/core_dev_tools/environment_status.h"
#include "src/lib/analytics/cpp/google_analytics_4/client.h"

namespace analytics::core_dev_tools::internal {

void PrepareGa4Client(google_analytics_4::Client& client, std::string tool_version,
                      std::string_view measurement_id, std::string_view measurement_key,
                      std::optional<BotInfo> bot = std::nullopt);

void PrepareGa4Client(google_analytics_4::Client& client, std::uint32_t tool_version,
                      std::string_view measurement_id, std::string_view measurement_key,
                      std::optional<BotInfo> bot = std::nullopt);

}  // namespace analytics::core_dev_tools::internal

#endif  // SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_ANALYTICS_INTERNAL_H_
