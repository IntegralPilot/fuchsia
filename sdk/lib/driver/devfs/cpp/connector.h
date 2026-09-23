// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVFS_CPP_CONNECTOR_H_
#define LIB_DRIVER_DEVFS_CPP_CONNECTOR_H_

#include <fidl/fuchsia.device.fs/cpp/fidl.h>

namespace driver_devfs {

// A helper class for implementing the Connector protocol.
// This class provides type-safety over the protocol that is being connected to,
// as the parent will get back a `fidl::ServerEnd<MyProtocol>`.
template <typename Protocol>
class Connector final : public fidl::WireServer<fuchsia_device_fs::Connector> {
 public:
  // Create the Connector. This callback will be called whenever a client attempts to
  // connect.
  explicit Connector(fit::function<void(fidl::ServerEnd<Protocol>)> connect_callback)
      : callback_(std::move(connect_callback)) {}

  zx::result<> Bind(async_dispatcher_t* dispatcher,
                    fidl::ServerEnd<fuchsia_device_fs::Connector> server) {
    if (binding_.has_value()) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    binding_.emplace(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
    return zx::ok();
  }

  zx::result<fidl::ClientEnd<fuchsia_device_fs::Connector>> Bind(async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_device_fs::Connector>::Create();
    if (zx::result result = Bind(dispatcher, std::move(server_end)); result.is_error()) {
      return result.take_error();
    }
    return zx::ok(std::move(client_end));
  }

  std::optional<fidl::ServerBinding<fuchsia_device_fs::Connector>>& binding() { return binding_; }

 private:
  void Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) override {
    callback_(fidl::ServerEnd<Protocol>(std::move(request->server)));
  }

  const fit::function<void(fidl::ServerEnd<Protocol>)> callback_;
  std::optional<fidl::ServerBinding<fuchsia_device_fs::Connector>> binding_;
};

}  // namespace driver_devfs

#endif  // LIB_DRIVER_DEVFS_CPP_CONNECTOR_H_
