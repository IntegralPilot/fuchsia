// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.examples.echo/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/zx/channel.h>

#include <string>

#include <ddktl/device.h>
#include <ddktl/unbind-txn.h>
#include <fbl/ref_ptr.h>

namespace {

class Device;
using DeviceParent = ddk::Device<Device, ddk::Unbindable>;

class Device : public DeviceParent, public fidl::WireServer<fidl_examples_echo::Echo> {
 public:
  static zx_status_t Bind(void* ctx, zx_device_t* parent) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    if (endpoints.is_error()) {
      return endpoints.status_value();
    }

    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    auto device = std::make_unique<Device>(parent, dispatcher);

    auto echo_handler = fit::bind_member(device.get(), &Device::EchoHandler);
    fidl_examples_echo::EchoService::InstanceHandler handler({.echo = std::move(echo_handler)});

    auto result =
        device->outgoing_dir_.AddService<fidl_examples_echo::EchoService>(std::move(handler));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to add service the outgoing directory");
      return result.status_value();
    }

    result = device->outgoing_dir_.Serve(std::move(endpoints->server));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to serve the outgoing directory");
      return result.status_value();
    }

    std::array offers = {
        fidl_examples_echo::EchoService::Name,
    };

    auto status = device->DdkAdd(ddk::DeviceAddArgs("parent")
                                     .set_flags(DEVICE_ADD_MUST_ISOLATE)
                                     .set_fidl_service_offers(offers)
                                     .set_outgoing_dir(endpoints->client.TakeChannel()));
    if (status == ZX_OK) {
      [[maybe_unused]] auto ptr = device.release();
    } else {
      zxlogf(ERROR, "Failed to add device");
      return status;
    }

    // The Driver Framework strictly enforces a 1-to-1 Service-to-Node invariant.
    // We cannot offer multiple services on the same node (e.g., EchoService and EchoService2)
    // because the framework generates topological bind properties for each service. Adding both
    // services to a single node would cause colliding and ambiguous bind rules. Thus, we must
    // instantiate a separate 'device2' child node specifically to host 'EchoService2'.
    auto endpoints2 = fidl::CreateEndpoints<fuchsia_io::Directory>();
    if (endpoints2.is_error()) {
      return endpoints2.status_value();
    }
    auto device2 = std::make_unique<Device>(parent, dispatcher);
    auto echo_handler2 = fit::bind_member(device2.get(), &Device::EchoHandler);
    fidl_examples_echo::EchoService2::InstanceHandler handler2({.echo = std::move(echo_handler2)});

    result =
        device2->outgoing_dir_.AddService<fidl_examples_echo::EchoService2>(std::move(handler2));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to add service the outgoing directory");
      return result.status_value();
    }

    result = device2->outgoing_dir_.Serve(std::move(endpoints2->server));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to serve the outgoing directory");
      return result.status_value();
    }

    std::array offers2 = {
        fidl_examples_echo::EchoService2::Name,
    };

    status = device2->DdkAdd(ddk::DeviceAddArgs("parent2")
                                 .set_flags(DEVICE_ADD_MUST_ISOLATE)
                                 .set_fidl_service_offers(offers2)
                                 .set_outgoing_dir(endpoints2->client.TakeChannel()));
    if (status == ZX_OK) {
      [[maybe_unused]] auto ptr = device2.release();
    } else {
      zxlogf(ERROR, "Failed to add device2");
    }

    return status;
  }

  Device(zx_device_t* parent, async_dispatcher_t* dispatcher)
      : DeviceParent(parent), outgoing_dir_(component::OutgoingDirectory(dispatcher)) {}

  void DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }
  void DdkRelease() { delete this; }

 private:
  void EchoHandler(fidl::ServerEnd<fidl_examples_echo::Echo> request) {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    fidl::BindServer(dispatcher, std::move(request), this);
  }
  void EchoString(EchoStringRequestView request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(request->value);
  }

  component::OutgoingDirectory outgoing_dir_;
};

static constexpr zx_driver_ops_t kDriverOps = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Device::Bind;
  return ops;
}();

}  // namespace

ZIRCON_DRIVER(fidl_service_test_parent, kDriverOps, "zircon", "0.1");
