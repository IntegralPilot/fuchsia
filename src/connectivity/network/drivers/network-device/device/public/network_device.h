// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_PUBLIC_NETWORK_DEVICE_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_PUBLIC_NETWORK_DEVICE_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl_driver/cpp/transport.h>
#include <lib/fit/function.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/thread.h>

#include <memory>

#include <fbl/alloc_checker.h>

namespace network {

namespace netdev = fuchsia_hardware_network;

// TODO(https://fxbug.dev/42083623): Remove this and related artifacts once all parents have
// migrated to FIDL.
class NetworkDeviceImplBinder {
 public:
  enum class Synchronicity { Sync, Async };
  virtual ~NetworkDeviceImplBinder() = default;
  virtual zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>> Bind() = 0;
  // Use this for binder specific teardown if needed. The return value indicates if the teardown
  // is synchronous or asynchronous. Call |on_teardown_complete| when an asynchronous teardown has
  // completed. If teardown is synchronous then |on_teardown_complete| should NOT be called, as seen
  // in the base implementation here.
  virtual Synchronicity Teardown(fit::callback<void()>&& on_teardown_complete) {
    return Synchronicity::Sync;
  }
};

struct DeviceInterfaceDispatchers {
  DeviceInterfaceDispatchers() = default;
  DeviceInterfaceDispatchers(fdf::UnsynchronizedDispatcher& impl,
                             fdf::UnsynchronizedDispatcher& ifc,
                             fdf::UnsynchronizedDispatcher& port)
      : impl_(impl), ifc_(ifc), port_(port) {}
  DeviceInterfaceDispatchers(const DeviceInterfaceDispatchers&) = default;
  DeviceInterfaceDispatchers& operator=(const DeviceInterfaceDispatchers&) = default;
  // Used for the NetworkDeviceImpl client as well as some async tasks and FIDL servers.
  fdf::UnownedUnsynchronizedDispatcher impl_;
  // Used to serve the the NetworkDeviceIfc protocol to vendor drivers.
  fdf::UnownedUnsynchronizedDispatcher ifc_;
  // Used for the NetworkPort client.
  fdf::UnownedUnsynchronizedDispatcher port_;
};

// A class that represents a set of owned dispatchers suitable for use with a
// NetworkDeviceInterface. The class owns the dispatchers and provides functionality for
// synchronously shutting down the dispatchers. Shutdown has to be explicitly invoked, if the object
// is destroyed without being shutdown assert will be triggered.
class OwnedDeviceInterfaceDispatchers {
 public:
  static zx::result<std::unique_ptr<OwnedDeviceInterfaceDispatchers>> Create();

  DeviceInterfaceDispatchers Unowned();

  void ShutdownSync();

 private:
  OwnedDeviceInterfaceDispatchers();

  fdf::UnsynchronizedDispatcher impl_;
  libsync::Completion impl_shutdown_;
  fdf::UnsynchronizedDispatcher ifc_;
  libsync::Completion ifc_shutdown_;
  fdf::UnsynchronizedDispatcher port_;
  libsync::Completion port_shutdown_;
};

class NetworkDeviceInterface {
 public:
  virtual ~NetworkDeviceInterface() = default;
  // Creates a new NetworkDeviceInterface that will bind to the provided parent. The multiple
  // dispatchers required should be owned externally so that components that use multiple instances
  // of NetworkDeviceInterface can re-use these dispatchers between instances. Otherwise those
  // components may run into the limitations on the number of dispatcher threads that can be
  // created.
  static zx::result<std::unique_ptr<NetworkDeviceInterface>> Create(
      const DeviceInterfaceDispatchers& dispatchers,
      std::unique_ptr<NetworkDeviceImplBinder>&& binder);

  // Tears down the NetworkDeviceInterface.
  // A NetworkDeviceInterface must not be destroyed until the callback provided to teardown is
  // triggered, doing so may cause an assertion error. Immediately destroying a NetworkDevice that
  // never succeeded Init is allowed.
  virtual void Teardown(fit::callback<void()>) = 0;

  // Binds the request channel req to this NetworkDeviceInterface. Requests will be handled on the
  // dispatcher given to the device on creation.
  virtual zx_status_t Bind(fidl::ServerEnd<netdev::Device> req) = 0;

  // Binds the request channel req to a port belonging to this NetworkDeviceInterface. Requests will
  // be handled on the dispatcher given to the device on creation.
  virtual zx_status_t BindPort(uint8_t port_id, fidl::ServerEnd<netdev::Port> req) = 0;

 protected:
  NetworkDeviceInterface() = default;
};

}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_PUBLIC_NETWORK_DEVICE_H_
