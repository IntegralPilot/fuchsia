// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/ddktl/tests/metadata-server-test/metadata-sender-test-driver/metadata_test_device.h"

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>

#include <bind/metadata_server_test_bind_library/cpp/bind.h>
#include <ddktl/device.h>
#include <ddktl/metadata_server.h>

namespace ddk::test {

zx_status_t MetadataTestDevice::Create(
    zx_device_t* parent, const char* device_name, const char* device_purpose,
    std::optional<fidl::ClientEnd<fuchsia_io::Directory>> outgoing) {
  auto device = std::make_unique<MetadataTestDevice>(parent);

  zx_status_t status = device->Init(device_name, device_purpose, std::move(outgoing));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to initialize: %s", zx_status_get_string(status));
    return status;
  }

  [[maybe_unused]] auto unused = device.release();
  return ZX_OK;
}

zx_status_t MetadataTestDevice::Init(
    const char* device_name, const char* device_purpose,
    std::optional<fidl::ClientEnd<fuchsia_io::Directory>> outgoing) {
  std::vector<zx_device_str_prop_t> device_string_properties = {
      ddk::MakeStrProperty(bind_metadata_server_test::PURPOSE, device_purpose),
  };

  std::array offers = {
      ddk::MetadataServer<fuchsia_hardware_test::wire::Metadata>::kFidlServiceName};
  ddk::DeviceAddArgs args{device_name};
  args.set_str_props(device_string_properties);
  if (outgoing.has_value()) {
    args.set_fidl_service_offers(offers).set_outgoing_dir(outgoing->TakeChannel());
  }

  zx_status_t status = DdkAdd(args);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add device: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

void MetadataTestDevice::DdkRelease() { delete this; }

}  // namespace ddk::test
