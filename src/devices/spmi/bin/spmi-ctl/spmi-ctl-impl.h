// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fidl/fuchsia.hardware.spmi/cpp/fidl.h>

#include <optional>

class SpmiCtl {
 public:
  explicit SpmiCtl(fidl::ClientEnd<fuchsia_hardware_spmi::Debug> debug) {
    test_client_.emplace(std::move(debug));  // Allows injecting the device from tests.
  }
  SpmiCtl() = default;

  int Execute(int argc, char** argv);

 private:
  fidl::SyncClient<fuchsia_hardware_spmi::Device> GetSpmiClient(std::string controller,
                                                                uint8_t target);
  void ListDevices();

  std::optional<fidl::SyncClient<fuchsia_hardware_spmi::Debug>> test_client_;
};
