// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "test_network_device_ifc.h"

#include <lib/fidl_driver/cpp/transport.h>

namespace netdriver = fuchsia_hardware_network_driver;

namespace wlan::drivers::components::test {

zx::result<fdf::ClientEnd<netdriver::NetworkDeviceIfc>> TestNetworkDeviceIfc::Bind(
    fdf_dispatcher_t* dispatcher) {
  auto endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::NetworkDeviceIfc>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }

  binding_ = fdf::BindServer(dispatcher, std::move(endpoints->server), this);

  return zx::ok(std::move(endpoints->client));
}

}  // namespace wlan::drivers::components::test
