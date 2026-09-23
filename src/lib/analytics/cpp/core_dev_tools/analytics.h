// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_ANALYTICS_H_
#define SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_ANALYTICS_H_

#include <memory>
#include <string>
#include <vector>

#include "sdk/lib/syslog/cpp/macros.h"
#include "src/lib/analytics/cpp/core_dev_tools/analytics_internal.h"
#include "src/lib/analytics/cpp/core_dev_tools/analytics_messages.h"
#include "src/lib/analytics/cpp/core_dev_tools/analytics_status.h"
#include "src/lib/analytics/cpp/core_dev_tools/command_line_options.h"
#include "src/lib/analytics/cpp/core_dev_tools/environment_status.h"
#include "src/lib/analytics/cpp/core_dev_tools/ga4_common_events.h"
#include "src/lib/analytics/cpp/core_dev_tools/google_analytics_4_client.h"
#include "src/lib/analytics/cpp/core_dev_tools/persistent_status.h"
#include "src/lib/analytics/cpp/google_analytics_4/testing_client.h"
#include "src/lib/analytics/cpp/metric_properties/metric_properties.h"

namespace analytics::core_dev_tools {

// This class uses template following the pattern of CRTP
// (See https://en.wikipedia.org/wiki/Curiously_recurring_template_pattern).
// We use CRTP instead of the more common dynamic polymorphism via inheritance, in order to
// provide a simple, static interface for analytics, such that sending an event just looks like a
// one-line command without creating an object first. We choose static interface here because the
// action of sending analytics itself is rather static, without interacting with any internal
// status that changes from instance to instance.
//
// To use this class, one must inherit this class and specify required constants like below:
//
//     class ToolAnalytics : public Analytics<ToolAnalytics> {
//      public:
//       // ......
//
//      private:
//       friend class Analytics<ToolAnalytics>;
//       static constexpr char kToolName[] = "tool";
//       static constexpr char kToolVersion[] = "1.0";
//       static constexpr int64_t kQuitTimeoutMs = 500; // wait for at most 500ms before quitting
//       static constexpr char kMeasurementId[] = "G-XXXXXXXXXX";
//       static constexpr char kMeasurementKey[] = "YYYYYYYYYYYYYY";
//       static constexpr char kEnableArgs[] = "--analytics=enable";
//       static constexpr char kDisableArgs[] = "--analytics=disable";
//       static constexpr char kStatusArgs[] = "--show-analytics";
//     }
//
// One also needs to (if not already) add the following lines to the main() function before any
// threads are spawned and any use of Curl or Analytics:
//     debug_ipc::Curl::GlobalInit();
//     auto deferred_cleanup_curl = fit::defer(debug_ipc::Curl::GlobalCleanup);
//     auto deferred_cleanup_analytics = fit::defer(Analytics::CleanUp);
// and include related headers, e.g. <lib/fit/defer.h> and "src/developer/debug/zxdb/common/curl.h".
//
// The derived class can also define their own functions for sending analytics. For example
//
//     // The definition of a static public function in ToolAnalytics
//     void ToolAnalytics::IfEnabledSendExitEvent() {
//       if(<runtime analytics enabled>) {
//         SendGoogleAnalyticsHit(<...>);
//       }
//     }
//
template <class T>
class Analytics {
 public:
  static void InitBotAware(AnalyticsOption analytics_option, bool enable_on_bots = false) {
    if (IsDisabledByEnvironment() || !metric_properties::HasHome()) {
      T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kDisabled);
      return;
    }
    metric_properties::MigrateMetricDirectory();
    BotInfo bot = GetBotInfo();
    if (bot.IsRunByBot()) {
      if (enable_on_bots && (internal::PersistentStatus::IsFirstLaunchOfFirstTool() ||
                             !internal::PersistentStatus::IsEnabled())) {
        internal::PersistentStatus::Enable();
      }
      T::SetRuntimeAnalyticsStatus(enable_on_bots ? AnalyticsStatus::kEnabled
                                                  : AnalyticsStatus::kDisabled);
    } else {
      Init(analytics_option);
    }

    if (enable_on_bots && IsEnabled()) {
      FX_DCHECK(!client_ga4_ && !client_is_cleaned_up_);
      CreateAndPrepareGa4Client(bot);
    }
  }

  static void PersistentEnable() {
    if (internal::PersistentStatus::IsEnabled()) {
      internal::ShowAlready(AnalyticsStatus::kEnabled);
    } else {
      internal::PersistentStatus::Enable();
      internal::ShowChangedTo(AnalyticsStatus::kEnabled);
      SendAnalyticsManualEnableEvent();
    }
  }

  static void PersistentDisable() {
    if (internal::PersistentStatus::IsEnabled()) {
      SendAnalyticsDisableEvent();
      internal::PersistentStatus::Disable();
      internal::ShowChangedTo(AnalyticsStatus::kDisabled);
    } else {
      internal::ShowAlready(AnalyticsStatus::kDisabled);
    }
  }

  // Show the persistent analytics status and the what is collected
  static void ShowAnalytics() {
    internal::ToolInfo tool_info{T::kToolName, T::kEnableArgs, T::kDisableArgs, T::kStatusArgs};
    internal::ShowAnalytics(tool_info, internal::PersistentStatus::IsEnabled()
                                           ? AnalyticsStatus::kEnabled
                                           : AnalyticsStatus::kDisabled);
  }

  static void IfEnabledSendInvokeEvent() {
    if (IsEnabled()) {
      SendGa4Event(std::make_unique<InvokeEvent>());
    }
  }

  static void IfEnabledSendGa4Event(std::unique_ptr<google_analytics_4::Event> event) {
    if (IsEnabled()) {
      SendGa4Event(std::move(event));
    }
  }

  static void IfEnabledSendGa4Events(
      std::vector<std::unique_ptr<google_analytics_4::Event>> events) {
    if (CanSend()) {
      client_ga4_->AddEvents(std::move(events));
    }
  }

  static void IfEnabledAddGa4EventToDefaultBatch(std::unique_ptr<google_analytics_4::Event> event) {
    if (CanSend()) {
      client_ga4_->AddEventToDefaultBatch(std::move(event));
    }
  }

  static void IfEnabledSendDefaultBatch() {
    if (CanSend()) {
      client_ga4_->SendDefaultBatch();
    }
  }

  static void CleanUp() {
    delete client_ga4_;
    client_ga4_ = nullptr;
    client_is_cleaned_up_ = true;
  }

  // For tests only. Do not use with other Init* functions.
  // With this init, data will not be sent to Google Analytics. But instead the
  // sender function will be called with the POST body (passed as `const
  // std::string&`), which is the JSON representation of a Measurement object.
  static void InitTestingClient(std::function<void(const std::string&)> sender) {
    client_ga4_ = new google_analytics_4::TestingClient(std::move(sender));
    T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kEnabled);
  }

 protected:
  static void SendGa4Event(std::unique_ptr<google_analytics_4::Event> event) {
    if (!client_is_cleaned_up_) {
      if (!client_ga4_) {
        CreateAndPrepareGa4Client();
      }
      client_ga4_->AddEvent(std::move(event));
    }
  }

  static bool CanSend() {
    if (!IsEnabled()) {
      return false;
    }
    if (!client_is_cleaned_up_) {
      if (!client_ga4_) {
        CreateAndPrepareGa4Client();
      }
      return true;
    }
    return false;
  }

  static bool ClientIsCleanedUp() { return client_is_cleaned_up_; }

  static void SetRuntimeAnalyticsStatus(AnalyticsStatus status) {
    enabled_runtime_ = (status == AnalyticsStatus::kEnabled);
  }

  static bool IsEnabled() { return !ClientIsCleanedUp() && enabled_runtime_; }

  inline static bool enabled_runtime_ = false;

 private:
  // Init analytics status, and show suitable welcome messages if on the first run.
  static void Init(AnalyticsOption analytics_option) {
    internal::PersistentStatus persistent_status(T::kToolName);
    if (internal::PersistentStatus::IsFirstLaunchOfFirstTool()) {
      InitFirstRunOfFirstTool(persistent_status);
    } else if (analytics_option == AnalyticsOption::kSubLaunchFirst) {
      InitSubLaunchedFirst();
    } else if (analytics_option == AnalyticsOption::kSubLaunchNormal) {
      InitSubLaunchedNormal();
    } else if (persistent_status.IsFirstDirectLaunch()) {
      InitFirstRunOfOtherTool(persistent_status);
    } else {
      InitSubsequentRun();
    }
  }

  static void InitFirstRunOfFirstTool(internal::PersistentStatus& persistent_status) {
    internal::ToolInfo tool_info{T::kToolName, T::kEnableArgs, T::kDisableArgs, T::kStatusArgs};
    ShowMessageFirstRunOfFirstTool(tool_info);
    internal::PersistentStatus::Enable();
    persistent_status.MarkAsDirectlyLaunched();
    T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kDisabled);
  }

  static void InitFirstRunOfOtherTool(internal::PersistentStatus& persistent_status) {
    internal::ToolInfo tool_info{T::kToolName, T::kEnableArgs, T::kDisableArgs, T::kStatusArgs};
    if (internal::PersistentStatus::IsEnabled()) {
      ShowMessageFirstRunOfOtherTool(tool_info, AnalyticsStatus::kEnabled);
      persistent_status.MarkAsDirectlyLaunched();
      T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kEnabled);
    } else {
      ShowMessageFirstRunOfOtherTool(tool_info, AnalyticsStatus::kDisabled);
      persistent_status.MarkAsDirectlyLaunched();
      T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kDisabled);
    }
  }

  static void InitSubsequentRun() {
    if (internal::PersistentStatus::IsEnabled()) {
      T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kEnabled);
    } else {
      T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kDisabled);
    }
  }

  static void InitSubLaunchedNormal() { InitSubsequentRun(); }

  static void InitSubLaunchedFirst() { T::SetRuntimeAnalyticsStatus(AnalyticsStatus::kDisabled); }

  static void CreateAndPrepareGa4Client(std::optional<BotInfo> bot = std::nullopt) {
    client_ga4_ = new Ga4Client(T::kQuitTimeoutMs);
    internal::PrepareGa4Client(*client_ga4_, T::kToolVersion, T::kMeasurementId, T::kMeasurementKey,
                               bot);
  }

  static void SendAnalyticsManualEnableEvent() {
    SendGa4Event(ChangeAnalyticsStatusEvent::CreateManuallyEnabledEvent());
  }

  static void SendAnalyticsDisableEvent() {
    SendGa4Event(ChangeAnalyticsStatusEvent::CreateDisabledEvent());
  }

  inline static bool client_is_cleaned_up_ = false;
  // Instead of using an fbl::NoDestructor<std::unique_ptr<google_analytics::Client>>, a raw pointer
  // is used here, since
  // (1) there is no ownership transfer
  // (2) the life time of the pointed-to object is managed manually
  // (3) using a raw pointer here makes code simpler and easier to read
  inline static google_analytics_4::Client* client_ga4_ = nullptr;
};

}  // namespace analytics::core_dev_tools

#endif  // SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_ANALYTICS_H_
