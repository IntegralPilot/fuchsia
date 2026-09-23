// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_TESTING_CLIENT_H_
#define SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_TESTING_CLIENT_H_

#include <functional>

#include "src/lib/analytics/cpp/google_analytics_4/client.h"

namespace analytics::google_analytics_4 {

// Help tool analytics implementers test their implementation
class TestingClient : public Client {
 public:
  explicit TestingClient(std::function<void(const std::string&)> sender)
      : sender_(std::move(sender)) {
    SetClientId("testing-client");
    SetQueryParameters("testing-id", "testing-key");
  }

 private:
  void SendData(std::string body) override { sender_(body); }
  std::function<void(const std::string&)> sender_;
};

}  // namespace analytics::google_analytics_4

#endif  // SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_TESTING_CLIENT_H_
