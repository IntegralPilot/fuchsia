// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "network_device.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/env.h>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

namespace network {
namespace {

constexpr char kDriverName[] = "network-device";
constexpr char kDevFsClassName[] = "network";
constexpr char kDevFsChildNodeName[] = "network-device";

}  // namespace

NetworkDevice::NetworkDevice() : fdf::DriverBase2(kDriverName) {}

NetworkDevice::~NetworkDevice() {
  if (dispatchers_) {
    dispatchers_->ShutdownSync();
  }
}

void NetworkDevice::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  zx::result<> result = [&]() -> zx::result<> {
    zx::result dispatchers = OwnedDeviceInterfaceDispatchers::Create();
    if (dispatchers.is_error()) {
      fdf::error("failed to create owned dispatchers: {}", dispatchers);
      return dispatchers.take_error();
    }
    dispatchers_ = std::move(dispatchers.value());

    zx::result<std::unique_ptr<NetworkDeviceImplBinder>> binder = CreateImplBinder(incoming);
    if (binder.is_error()) {
      fdf::error("failed to create network device binder: {}", binder);
      return binder.take_error();
    }

    zx::result device =
        NetworkDeviceInterface::Create(dispatchers_->Unowned(), std::move(binder.value()));
    if (device.is_error()) {
      fdf::error("failed to create inner device {}", device);
      return device.take_error();
    }
    device_ = std::move(device.value());

    // Create a devfs connector and child node for netcfg to discover and connect to.
    zx::result connector = devfs_connector_.Bind(dispatcher());
    if (connector.is_error()) {
      fdf::error("failed to bind devfs connector: {}", connector);
      return connector.take_error();
    }

    fuchsia_driver_framework::DevfsAddArgs devfs_args;
    devfs_args.connector(std::move(connector.value()))
        .class_name(kDevFsClassName)
        .connector_supports(fuchsia_device_fs::ConnectionType::kController);

    // Use AddOwnedChild to prevent other drivers from binding to the node. The node only exists for
    // netcfg to discover and connect to, no other drivers are involved.
    zx::result child = AddOwnedChild(kDevFsChildNodeName, devfs_args);
    if (child.is_error()) {
      fdf::error("failed to add child node: {}", child);
      return child.take_error();
    }

    child_node_.Bind(std::move(child->node_));
    return zx::ok();
  }();

  if (result.is_error() && device_) {
    // Start failed but got to the point where the device was created. We must tear it down.
    // PrepareStop will not be called if Start fails.
    device_->Teardown([result, completer = std::move(completer)]() mutable { completer(result); });
  } else {
    completer(result);
  }
}

void NetworkDevice::Stop(fdf::StopCompleter completer) {
  fdf::info("{:p} Stop", static_cast<const void*>(this));
  device_->Teardown([completer = std::move(completer), this]() mutable {
    fdf::info("{:p} Stop Done", static_cast<const void*>(this));
    completer(zx::ok());
  });
}

void NetworkDevice::Connect(fidl::ServerEnd<fuchsia_hardware_network::Device> request) {
  ZX_ASSERT_MSG(device_, "can't serve device if not bound to parent implementation");
  device_->Bind(std::move(request));
}

zx::result<std::unique_ptr<NetworkDeviceImplBinder>> NetworkDevice::CreateImplBinder(
    const std::shared_ptr<fdf::Namespace>& incoming) {
  fbl::AllocChecker ac;

  std::unique_ptr fidl = fbl::make_unique_checked<FidlNetworkDeviceImplBinder>(&ac, incoming);
  if (!ac.check()) {
    fdf::error("no memory");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(fidl));
}

zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>>
FidlNetworkDeviceImplBinder::Bind() {
  if (!incoming_) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  zx::result client_end =
      incoming_->Connect<fuchsia_hardware_network_driver::Service::NetworkDeviceImpl>();
  if (client_end.is_error()) {
    fdf::error("failed to connect to parent device: {}", client_end);
    return client_end;
  }
  return client_end;
}

}  // namespace network

FUCHSIA_DRIVER_EXPORT2(::network::NetworkDevice);
