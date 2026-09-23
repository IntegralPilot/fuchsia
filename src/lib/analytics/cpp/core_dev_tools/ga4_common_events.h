// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_GA4_COMMON_EVENTS_H_
#define SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_GA4_COMMON_EVENTS_H_

#include <memory>

#include "src/lib/analytics/cpp/google_analytics_4/event.h"

namespace analytics::core_dev_tools {

class ChangeAnalyticsStatusEvent : public google_analytics_4::Event {
 public:
  static auto CreateDisabledEvent() {
    return std::unique_ptr<ChangeAnalyticsStatusEvent>(new ChangeAnalyticsStatusEvent("disabled"));
  }
  static auto CreateManuallyEnabledEvent() {
    return std::unique_ptr<ChangeAnalyticsStatusEvent>(
        new ChangeAnalyticsStatusEvent("manually_enabled"));
  }

 private:
  explicit ChangeAnalyticsStatusEvent(std::string analytics_status)
      : google_analytics_4::Event("change_analytics_status") {
    SetParameter("analytics_status", std::move(analytics_status));
  }
};

class InvokeEvent : public google_analytics_4::Event {
 public:
  InvokeEvent() : google_analytics_4::Event("invoke") {}
};

}  // namespace analytics::core_dev_tools

#endif  // SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_GA4_COMMON_EVENTS_H_
