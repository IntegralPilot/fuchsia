// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/fpromise/result.h>

#include <iostream>

#include "src/developer/debug/zxdb/common/curl.h"
#include "src/lib/analytics/cpp/core_dev_tools/google_analytics_4_client.h"

using ::analytics::core_dev_tools::Ga4Client;
using ::analytics::core_dev_tools::Ga4Event;

class TestEvent : public Ga4Event {
 public:
  TestEvent() : Ga4Event("test_event") {
    SetParameter("p1", "v1");
    SetParameter("p2", 2);
    SetParameter("p3", false);
    SetParameter("p4", 2.5);
  }
};

int main(int argc, char* argv[]) {
  if (argc != 3) {
    std::cerr << "Usage: " << argv[0] << " <measurement-id> <measurement-key>" << std::endl;
    return 1;
  }

  std::string measurement_id(argv[1]);
  std::string measurement_key(argv[2]);

  zxdb::Curl::GlobalInit();
  auto deferred_cleanup_curl = fit::defer(zxdb::Curl::GlobalCleanup);

  Ga4Client ga_client(-1);
  ga_client.SetQueryParameters(measurement_id, measurement_key);
  ga_client.SetClientId("test-client");
  ga_client.SetUserProperty("up1", "v1");
  ga_client.SetUserProperty("up2", 2);
  ga_client.SetUserProperty("up3", false);
  ga_client.SetUserProperty("up4", 2.5);

  ga_client.AddEvent(std::make_unique<TestEvent>());

  return 0;
}
