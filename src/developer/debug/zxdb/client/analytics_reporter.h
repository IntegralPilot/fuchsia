// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ANALYTICS_REPORTER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ANALYTICS_REPORTER_H_

#include <chrono>
#include <string>

#include "src/developer/debug/zxdb/client/analytics_event.h"

namespace zxdb {

class Session;

// Entry point for sending zxdb events. This class is owned by the Session, and can be used from
// different contexts where interesting things happen. The goal of the API is to be as simple and
// unobtrusive as possible for the caller. There are no callbacks and no status reporting. Callers
// may safely fire and forget any event, and do not need to perform any checks for validity or
// enablement to call these methods.
//
// See the corresponding event definitions for more information about the parameters.
class AnalyticsReporter {
 public:
  AnalyticsReporter();

  // Explicitly initialize analytics reporting. This must be called before any of the reporting
  // methods will actually fire events. The session is sent with all events to the analytics client
  // implementation (see analytics.h), where the session is checked for validity. If an event
  // requires access to the Session object, it should be passed directly to that Event's respective
  // Report method.
  //
  // Explicit opt-in is chosen so that clients of this library that are not zxdb may simply ignore
  // this class and not impact zxdb's analytics. Other users of this library (symbolizer) would skew
  // other tools' data. The session is also manually constructed in many unittests, which should not
  // be reporting analytics.
  //
  // Note that this is separate from the user opt-in, which is handled by the analytics library.
  void Init(Session* session);

  void ReportInvoked() const;
  void ReportSessionStarted() const;
  void ReportSessionConnected(bool is_minidump, bool local_agent) const;
  void ReportSessionEnded() const;
  void ReportCommand(const CommandReport& report) const;
  void ReportConsoleType(ConsoleType::Type type) const;

 private:
  Session* session_ = nullptr;  // non-owning.
  const std::string session_id_;
  // This is set when Init is called.
  std::chrono::time_point<std::chrono::steady_clock> start_time_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ANALYTICS_REPORTER_H_
