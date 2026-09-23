// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/ddktl/tests/metadata-server-test/metadata-sender-test-driver/metadata_sender_test_device.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>

#include <sstream>

#include <bind/metadata_server_test_bind_library/cpp/bind.h>

#include "src/lib/ddktl/tests/metadata-server-test/metadata-sender-test-driver/metadata_test_device.h"

namespace ddk::test {

zx_status_t MetadataSenderTestDevice::Init() {
  zx_status_t status = metadata_server_.Serve(outgoing_, dispatcher_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to serve metadata: %s", zx_status_get_string(status));
    return status;
  }

  status = DdkAdd(DeviceAddArgs("metadata_sender").set_flags(DEVICE_ADD_NON_BINDABLE));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add device: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> MetadataSenderTestDevice::ServeOutgoing() {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    zxlogf(ERROR, "Failed to create endpoints: %s", endpoints.status_string());
    return endpoints.take_error();
  }

  zx::result result = outgoing_.Serve(std::move(endpoints->server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to serve outgoing directory: %s", result.status_string());
    return result.take_error();
  }

  return zx::ok(std::move(endpoints->client));
}

zx_status_t MetadataSenderTestDevice::Create(void* ctx, zx_device_t* parent, const char* name,
                                             zx_handle_t handle) {
  auto device = std::make_unique<MetadataSenderTestDevice>(parent);

  zx_status_t status = device->Init();
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to initialize: %s", zx_status_get_string(status));
    return status;
  }

  [[maybe_unused]] auto unused = device.release();
  return ZX_OK;
}

void MetadataSenderTestDevice::DdkRelease() { delete this; }

void MetadataSenderTestDevice::SetMetadata(SetMetadataRequestView request,
                                           SetMetadataCompleter::Sync& completer) {
  zx_status_t status = metadata_server_.SetMetadata(request->metadata);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to set metadata: %s", zx_status_get_string(status));
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess();
}

void MetadataSenderTestDevice::AddMetadataRetrieverDevice(
    AddMetadataRetrieverDeviceRequestView request,
    AddMetadataRetrieverDeviceCompleter::Sync& completer) {
  zx::result device_name = AddMetadataDevice(bind_metadata_server_test::PURPOSE_RETRIEVE_METADATA,
                                             request->expose_metadata);
  if (device_name.is_error()) {
    zxlogf(ERROR, "Failed to add metadata device: %s", device_name.status_string());
    completer.ReplyError(device_name.status_value());
    return;
  }
  completer.ReplySuccess(fidl::StringView::FromExternal(device_name.value()));
}

void MetadataSenderTestDevice::AddMetadataForwarderDevice(
    AddMetadataForwarderDeviceCompleter::Sync& completer) {
  zx::result device_name =
      AddMetadataDevice(bind_metadata_server_test::PURPOSE_FORWARD_METADATA, true);
  if (device_name.is_error()) {
    zxlogf(ERROR, "Failed to add metadata device: %s", device_name.status_string());
    completer.ReplyError(device_name.status_value());
    return;
  }
  completer.ReplySuccess(fidl::StringView::FromExternal(device_name.value()));
}

zx::result<std::string> MetadataSenderTestDevice::AddMetadataDevice(const char* device_purpose,
                                                                    bool expose_metadata) {
  std::stringstream device_name_builder;
  device_name_builder << "metadata_" << num_metadata_devices_;
  std::string device_name = device_name_builder.str();

  std::optional<fidl::ClientEnd<fuchsia_io::Directory>> outgoing;
  if (expose_metadata) {
    zx::result result = ServeOutgoing();
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to initialize outgoing directory: %s", result.status_string());
      return result.take_error();
    }
    outgoing.emplace(std::move(result.value()));
  }

  zx_status_t status =
      MetadataTestDevice::Create(zxdev(), device_name.c_str(), device_purpose, std::move(outgoing));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create metadata device: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(std::move(device_name));
}

static constexpr zx_driver_ops_t sender_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.create = &MetadataSenderTestDevice::Create;
  return ops;
}();

}  // namespace ddk::test

ZIRCON_DRIVER(metadata_sender, ddk::test::sender_driver_ops, "zircon", "0.1");
