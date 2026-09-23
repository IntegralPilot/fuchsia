// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/analytics.h"

#include "src/developer/debug/zxdb/client/setting_schema_definition.h"

namespace zxdb {

using ::analytics::core_dev_tools::AnalyticsOption;

void Analytics::Init(Session& session, AnalyticsOption analytics_option) {
  InitBotAware(analytics_option, false);
  session.system().settings().SetBool(ClientSettings::System::kEnableAnalytics, enabled_runtime_);
}

bool Analytics::IsEnabled(Session* session) {
  return !ClientIsCleanedUp() &&
         session->system().settings().GetBool(ClientSettings::System::kEnableAnalytics);
}

void Analytics::IfEnabledSendEvent(Session* session,
                                   std::unique_ptr<analytics::core_dev_tools::Ga4Event> event) {
  // The AnalyticsReporter will send us a NULL session if it has not been initialized.
  if (session && IsEnabled(session)) {
    SendGa4Event(std::move(event));
  }
}

}  // namespace zxdb
