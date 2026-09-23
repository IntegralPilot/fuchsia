// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_NETWORK_DEVICE_CLIENT_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_NETWORK_DEVICE_CLIENT_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fidl_driver/cpp/wire_client.h>
#include <lib/zx/result.h>

namespace wlan::drivers::components::test {

// A test support class that discovers the NetworkDeviceImpl service provided by the component
// library's NetworkDevice class. Once Initialize has been called on a NetworkDevice object the
// service should be published in an outgoing directory and this class should be able to discover
// it. Calls can then be made to the NetworkDevice implementation using the client exposed in this
// class.
class NetworkDeviceClient {
 public:
  NetworkDeviceClient();
  NetworkDeviceClient(NetworkDeviceClient&&) noexcept;

  NetworkDeviceClient& operator=(NetworkDeviceClient&&) noexcept;

  // Create a client for a Network Device that has been made available on the server end of
  // |driver_outgoing|.
  static zx::result<NetworkDeviceClient> Create(
      fidl::UnownedClientEnd<fuchsia_io::Directory> driver_outgoing, fdf_dispatcher_t* dispatcher);

  bool is_valid() const { return client_.is_valid(); }

  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceImpl>* operator->() {
    return &client_;
  }
  const fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceImpl>* operator->()
      const {
    return &client_;
  }

 private:
  zx_status_t Connect(fidl::UnownedClientEnd<fuchsia_io::Directory> driver_outgoing,
                      fdf_dispatcher_t* dispatcher);

  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceImpl> client_;
};

}  // namespace wlan::drivers::components::test

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_NETWORK_DEVICE_CLIENT_H_
