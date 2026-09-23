// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/analytics_reporter.h"

#include <chrono>

#include "src/developer/debug/zxdb/client/analytics.h"
#include "src/developer/debug/zxdb/client/analytics_event.h"
#include "src/lib/uuid/uuid.h"

namespace zxdb {

AnalyticsReporter::AnalyticsReporter() : session_id_(uuid::Generate()) {}

void AnalyticsReporter::Init(Session* session) {
  session_ = session;
  start_time_ = std::chrono::steady_clock::now();
}

void AnalyticsReporter::ReportInvoked() const {
  Analytics::IfEnabledSendEvent(session_,
                                std::make_unique<analytics::core_dev_tools::InvokeEvent>());
}

void AnalyticsReporter::ReportSessionStarted() const {
  Analytics::IfEnabledSendEvent(session_, std::make_unique<SessionStarted>(session_id_));
}

void AnalyticsReporter::ReportSessionConnected(bool is_minidump, bool local_agent) const {
  auto connected = std::make_unique<SessionConnected>(session_id_);

  // Check for minidump first, since this will also set System::Where to local and we want to
  // differentiate between a locally running DebugAgent and a minidump.
  if (is_minidump) {
    connected->SetRemoteType(SessionConnected::RemoteType::kMinidump);
  } else if (local_agent) {
    connected->SetRemoteType(SessionConnected::RemoteType::kLocalAgent);
  } else {
    connected->SetRemoteType(SessionConnected::RemoteType::kRemoteAgent);
  }

  Analytics::IfEnabledSendEvent(session_, std::move(connected));
}

void AnalyticsReporter::ReportSessionEnded() const {
  auto session_ended = std::make_unique<SessionEnded>(session_id_);

  auto session_time = std::chrono::steady_clock::now() - start_time_;
  session_ended->SetSessionTime(
      std::chrono::duration_cast<std::chrono::milliseconds>(session_time));

  Analytics::IfEnabledSendEvent(session_, std::move(session_ended));
}

void AnalyticsReporter::ReportCommand(const CommandReport& report) const {
  auto command_event = std::make_unique<CommandEvent>(session_id_);
  command_event->FromCommandReport(report);

  Analytics::IfEnabledSendEvent(session_, std::move(command_event));
}

void AnalyticsReporter::ReportConsoleType(ConsoleType::Type type) const {
  auto console_type = std::make_unique<ConsoleType>(session_id_);
  console_type->SetConsoleType(type);

  Analytics::IfEnabledSendEvent(session_, std::move(console_type));
}

}  // namespace zxdb
