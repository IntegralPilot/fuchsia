// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/core_dev_tools/analytics_internal.h"

#include "src/lib/analytics/cpp/core_dev_tools/persistent_status.h"
#include "src/lib/analytics/cpp/core_dev_tools/system_info.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/substitute.h"

namespace analytics::core_dev_tools::internal {

void PrepareGa4Client(google_analytics_4::Client& client, std::string tool_version,
                      std::string_view measurement_id, std::string_view measurement_key,
                      std::optional<BotInfo> bot) {
  client.SetQueryParameters(measurement_id, measurement_key);
  client.SetClientId(internal::PersistentStatus::GetUuid());
  if (bot.has_value()) {
    client.SetUserProperty("bot", bot->IsRunByBot());
  } else {
    client.SetUserProperty("bot", false);
  }
  client.SetUserProperty("version", tool_version);
  auto system_info = GetSystemInfo();
  client.SetUserProperty("os", system_info.os);
  client.SetUserProperty("arch", system_info.arch);
}

void PrepareGa4Client(google_analytics_4::Client& client, std::uint32_t tool_version,
                      std::string_view measurement_id, std::string_view measurement_key,
                      std::optional<BotInfo> bot) {
  PrepareGa4Client(client, fxl::NumberToString(tool_version), measurement_id, measurement_key, bot);
}

}  // namespace analytics::core_dev_tools::internal
