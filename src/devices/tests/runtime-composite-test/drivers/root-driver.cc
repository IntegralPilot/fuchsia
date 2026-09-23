// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/runtime-composite-test/drivers/root-driver.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>

#include <bind/composite/test/lib/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>

namespace bind_test = bind_composite_test_lib;

namespace frct = fuchsia_runtime_composite_test;

namespace root_driver {

zx_status_t RootDriver::Bind(void* ctx, zx_device_t* dev) {
  auto root_dev = std::make_unique<RootDriver>(dev);
  zx_status_t status = root_dev->DdkAdd(ddk::DeviceAddArgs("root"));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto ptr = root_dev.release();

  // Add 2 children that matches the first composite node spec node.
  zx_device_str_prop_t str_fragment_props[] = {
      {bind_test::FLAG, str_prop_bool_val(true)},
  };

  auto dispatcher = fdf::Dispatcher::GetCurrent();
  auto fragment_dev_a = std::make_unique<RootDriver>(dev, dispatcher->get());
  auto client_end = fragment_dev_a->RegisterRuntimeService();
  if (client_end.is_error()) {
    return client_end.status_value();
  }
  std::array offers = {
      frct::Service::Name,
  };
  status = fragment_dev_a->DdkAdd(ddk::DeviceAddArgs("composite_fragment_a")
                                      .set_str_props(str_fragment_props)
                                      .set_proto_id(bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD)
                                      .set_runtime_service_offers(offers)
                                      .set_outgoing_dir(client_end->TakeChannel()));

  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto fragment_a_ptr = fragment_dev_a.release();

  // Add the leaf device.
  zx_device_str_prop_t leaf_props[] = {
      ddk::MakeStrProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_DEVICE),
  };

  auto leaf_dev = std::make_unique<RootDriver>(dev);
  status = leaf_dev->DdkAdd(ddk::DeviceAddArgs("leaf")
                                .set_str_props(leaf_props)
                                .set_proto_id(bind_fuchsia_test::BIND_PROTOCOL_DEVICE));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto leaf_ptr = leaf_dev.release();

  // Add 2 devices that matches the other composite node spec node.
  zx_device_str_prop_t str_fragment_props_2[] = {
      {bind_test::TEST_PROP, str_prop_str_val(bind_test::TEST_PROP_VALUE_2)},
  };

  auto fragment_dev_b = std::make_unique<RootDriver>(dev);
  status = fragment_dev_b->DdkAdd(ddk::DeviceAddArgs("composite_fragment_b")
                                      .set_str_props(str_fragment_props_2)
                                      .set_proto_id(bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto fragment_b_ptr = fragment_dev_b.release();

  return ZX_OK;
}

void RootDriver::DdkRelease() { delete this; }

void RootDriver::Handshake(HandshakeCompleter::Sync& completer) { completer.Reply(fit::ok()); }

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> RootDriver::RegisterRuntimeService() {
  auto protocol = [this](fdf::ServerEnd<frct::RuntimeCompositeProtocol> server_end) mutable {
    fdf::BindServer(fdf::Dispatcher::GetCurrent()->get(), std::move(server_end), this);
  };
  frct::Service::InstanceHandler handler({.runtime_composite_protocol = std::move(protocol)});

  auto add_status = outgoing_->AddService<frct::Service>(std::move(handler));
  if (add_status.is_error()) {
    return add_status.take_error();
  }
  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto result = outgoing_->Serve(std::move(endpoints->server));
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::move(endpoints->client));
}

static zx_driver_ops_t root_driver_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = RootDriver::Bind;
  return ops;
}();

}  // namespace root_driver

ZIRCON_DRIVER(RootDriver, root_driver::root_driver_driver_ops, "zircon", "0.1");
