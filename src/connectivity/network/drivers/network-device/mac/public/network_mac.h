// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_MAC_PUBLIC_NETWORK_MAC_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_MAC_PUBLIC_NETWORK_MAC_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>

#include <memory>

#include <fbl/alloc_checker.h>

namespace network {

namespace netdev = fuchsia_hardware_network;

class MacAddrDeviceInterface {
 public:
  using OnCreated = fit::callback<void(zx::result<std::shared_ptr<MacAddrDeviceInterface>>)>;

  virtual ~MacAddrDeviceInterface() = default;

  // Creates a new MacAddrDeviceInterface that is bound to the provided parent. The creation process
  // will make asynchronous calls into the provided parent and the resulting object will be returned
  // asynchronously through a callback. Note that on an error the callback may be (but is not
  // guaranteed to be) called inline from the Create call. Because of this it's a good idea to avoid
  // acquiring locks in the on_created callback when an error is reported.
  static void Create(fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> parent,
                     fdf_dispatcher_t* dispatcher, OnCreated&& on_created);

  // Binds the request channel req to this MacAddrDeviceInterface. Requests will be handled on the
  // provided dispatcher.
  virtual zx_status_t Bind(async_dispatcher_t* dispatcher,
                           fidl::ServerEnd<netdev::MacAddressing> req) = 0;

  // Tears down this device, closing all bound FIDL clients.
  // It is safe to destroy this `MacAddrDeviceInterface` instance only once the callback is invoked.
  virtual void Teardown(fit::callback<void()> callback) = 0;

 protected:
  MacAddrDeviceInterface() = default;
};

}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_MAC_PUBLIC_NETWORK_MAC_H_
