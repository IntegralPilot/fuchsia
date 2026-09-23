// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/ddktl/tests/metadata-server-test/metadata-forwarder-test-driver/metadata_forwarder_test_device.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>

#include <bind/metadata_server_test_bind_library/cpp/bind.h>

namespace ddk::test {

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> MetadataForwarderTestDevice::InitOutgoing() {
  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    zxlogf(ERROR, "Failed to create endpoints: %s", endpoints.status_string());
    return endpoints.take_error();
  }

  zx_status_t status = metadata_server_.Serve(outgoing_, dispatcher_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to serve metadata: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  zx::result result = outgoing_.Serve(std::move(endpoints->server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to serve outgoing directory: %s", result.status_string());
    return result.take_error();
  }

  return zx::ok(std::move(endpoints->client));
}

zx_status_t MetadataForwarderTestDevice::Bind(void* ctx, zx_device_t* dev) {
  auto metadata_forwarder_test_driver = std::make_unique<MetadataForwarderTestDevice>(dev);

  zx::result outgoing = metadata_forwarder_test_driver->InitOutgoing();
  if (outgoing.is_error()) {
    zxlogf(ERROR, "Failed to initialize outgoing directory: %s", outgoing.status_string());
    return outgoing.status_value();
  }

  std::array offers = {MetadataServer<fuchsia_hardware_test::Metadata>::kFidlServiceName};
  const std::vector<zx_device_str_prop_t> kStrProps = {
      ddk::MakeStrProperty(bind_metadata_server_test::PURPOSE,
                           bind_metadata_server_test::PURPOSE_RETRIEVE_METADATA),
  };

  zx_status_t status =
      metadata_forwarder_test_driver->DdkAdd(ddk::DeviceAddArgs("metadata_forwarder")
                                                 .set_fidl_service_offers(offers)
                                                 .set_str_props(kStrProps)
                                                 .set_outgoing_dir(outgoing->TakeChannel()));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add device: %s", zx_status_get_string(status));
    return status;
  }

  [[maybe_unused]] auto unused = metadata_forwarder_test_driver.release();
  return ZX_OK;
}

void MetadataForwarderTestDevice::DdkRelease() { delete this; }

void MetadataForwarderTestDevice::ForwardMetadata(ForwardMetadataCompleter::Sync& completer) {
  zx_status_t status = metadata_server_.ForwardMetadata(parent_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to forward metadata: %s", zx_status_get_string(status));
    completer.Reply(fit::error(status));
    return;
  }
  completer.Reply(fit::ok());
}

static constexpr zx_driver_ops_t forwarder_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = &MetadataForwarderTestDevice::Bind;
  return ops;
}();

}  // namespace ddk::test

ZIRCON_DRIVER(metadata_forwarder, ddk::test::forwarder_driver_ops, "zircon", "0.1");
