// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/core_dev_tools/google_analytics_4_client.h"

#include <lib/fpromise/bridge.h>
#include <lib/syslog/cpp/macros.h>

#include "src/developer/debug/zxdb/common/curl.h"
#include "src/lib/fxl/strings/substitute.h"

namespace analytics::core_dev_tools {

using ::zxdb::Curl;

namespace {

fxl::RefPtr<Curl> PrepareCurl(const std::string& url) {
  auto curl = fxl::MakeRefCounted<Curl>();

  curl->SetURL(url);
  curl->headers().emplace_back("Content-Type: application/json");

  return curl;
}

bool IsResponseCodeSuccess(long response_code) {
  return response_code >= 200 && response_code < 300;
}

fpromise::promise<> CurlPerformAsync(const fxl::RefPtr<Curl>& curl) {
  FX_DCHECK(curl);

  fpromise::bridge bridge;
  curl->Perform([completer = std::move(bridge.completer)](Curl* curl, Curl::Error result) mutable {
    auto response_code = curl->ResponseCode();
    if (!result && IsResponseCodeSuccess(response_code)) {
      completer.complete_ok();
    } else {
      completer.complete_error();
    }
  });
  return bridge.consumer.promise();
}

}  // namespace

void Ga4Client::SendData(std::string body) {
  executor_.schedule_task(fpromise::make_promise([this, body{std::move(body)}] {
    auto curl = PrepareCurl(url());
    curl->set_post_data(body);
    return CurlPerformAsync(curl);
  }));
}

}  // namespace analytics::core_dev_tools
