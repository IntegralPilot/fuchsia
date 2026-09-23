// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_GOOGLE_ANALYTICS_4_CLIENT_H_
#define SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_GOOGLE_ANALYTICS_4_CLIENT_H_

#include "src/lib/analytics/cpp/core_dev_tools/analytics_executor.h"
#include "src/lib/analytics/cpp/google_analytics_4/client.h"
#include "src/lib/analytics/cpp/google_analytics_4/event.h"

namespace analytics::core_dev_tools {

// Forwarding types from analytics::google_analytics_4
using Ga4Event = ::analytics::google_analytics_4::Event;

// To use this client, one needs to (if not already) add the following lines to
// the main() function before any threads are spawned and any use of Curl or
// this client:
//     zxdb::Curl::GlobalInit();
//     auto deferred_cleanup_curl = fit::defer(zxdb::Curl::GlobalCleanup);
// and include related headers, e.g. <lib/fit/defer.h> and
// "src/developer/debug/zxdb/common/curl.h".
class Ga4Client : public google_analytics_4::Client {
 public:
  explicit Ga4Client(int64_t quit_timeout_ms) : executor_(quit_timeout_ms) {}
  Ga4Client() : Ga4Client(0) {}

 private:
  void SendData(std::string body) override;
  AnalyticsExecutor executor_;
};

}  // namespace analytics::core_dev_tools

#endif  // SRC_LIB_ANALYTICS_CPP_CORE_DEV_TOOLS_GOOGLE_ANALYTICS_4_CLIENT_H_
