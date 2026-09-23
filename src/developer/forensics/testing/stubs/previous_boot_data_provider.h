// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_PREVIOUS_BOOT_DATA_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_PREVIOUS_BOOT_DATA_PROVIDER_H_

#include <fuchsia/diagnostics/persistence/cpp/fidl.h>
#include <fuchsia/diagnostics/persistence/cpp/fidl_test_base.h>

#include <utility>

#include "src/developer/forensics/testing/stubs/fidl_server_hlcpp.h"

namespace forensics::stubs {

namespace fpersistence = fuchsia::diagnostics::persistence;

class PreviousBootDataProviderBase
    : public SINGLE_BINDING_STUB_FIDL_SERVER(fuchsia::diagnostics::persistence,
                                             PreviousBootDataProvider) {};

class PreviousBootDataProviderReturnsData : public PreviousBootDataProviderBase {
 public:
  explicit PreviousBootDataProviderReturnsData(fpersistence::PreviousBootData data)
      : data_(std::move(data)) {}

  void WatchPreviousBootData(fpersistence::PreviousBootDataProviderOptions options,
                             WatchPreviousBootDataCallback callback) override {
    fpersistence::PreviousBootDataProvider_WatchPreviousBootData_Response response;
    response.data = std::move(data_);
    callback(fpersistence::PreviousBootDataProvider_WatchPreviousBootData_Result::WithResponse(
        std::move(response)));
  }

 private:
  fpersistence::PreviousBootData data_;
};

class PreviousBootDataProviderClosesConnection : public PreviousBootDataProviderBase {
 public:
  void WatchPreviousBootData(fpersistence::PreviousBootDataProviderOptions options,
                             WatchPreviousBootDataCallback callback) override {
    CloseConnection();
  }
};

class PreviousBootDataProviderNeverReturns : public PreviousBootDataProviderBase {
 public:
  void WatchPreviousBootData(fpersistence::PreviousBootDataProviderOptions options,
                             WatchPreviousBootDataCallback callback) override {}
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_PREVIOUS_BOOT_DATA_PROVIDER_H_
