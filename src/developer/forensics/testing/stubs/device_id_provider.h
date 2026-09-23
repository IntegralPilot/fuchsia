// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DEVICE_ID_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DEVICE_ID_PROVIDER_H_

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <fidl/fuchsia.feedback/cpp/test_base.h>

#include <optional>
#include <string>

#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics {
namespace stubs {

class DeviceIdProviderBase : public SingleBindingFidlServer<fuchsia_feedback::DeviceIdProvider> {
 public:
  void SetDeviceId(std::string device_id);

  // |fuchsia_feedback::DeviceIdProvider|
  void GetId(GetIdCompleter::Sync& completer) override;

 protected:
  DeviceIdProviderBase() : device_id_(std::nullopt) {}
  explicit DeviceIdProviderBase(const std::string& device_id) : device_id_(device_id) {}

  void GetIdInternal(GetIdCompleter::Sync& completer);

 private:
  std::optional<std::string> device_id_;

  std::optional<GetIdCompleter::Async> completer_;
  bool dirty_{true};
};

class DeviceIdProvider : public DeviceIdProviderBase {
 public:
  explicit DeviceIdProvider(const std::string& device_id) : DeviceIdProviderBase(device_id) {}
};

class DeviceIdProviderNeverReturns : public DeviceIdProviderBase {
 public:
  // |fuchsia_feedback::DeviceIdProvider|
  void GetId(GetIdCompleter::Sync& completer) override { completer_ = completer.ToAsync(); }

 private:
  std::optional<GetIdCompleter::Async> completer_;
};

class DeviceIdProviderReturnsError : public DeviceIdProviderBase {
 public:
  explicit DeviceIdProviderReturnsError(zx_status_t error) : error_(error) {}

  // |fuchsia_feedback::DeviceIdProvider|
  void GetId(GetIdCompleter::Sync& completer) override;

 private:
  zx_status_t error_;
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DEVICE_ID_PROVIDER_H_
