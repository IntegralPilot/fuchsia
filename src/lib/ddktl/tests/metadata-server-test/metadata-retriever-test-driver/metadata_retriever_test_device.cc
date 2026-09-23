// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/ddktl/tests/metadata-server-test/metadata-retriever-test-driver/metadata_retriever_test_device.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>

#include <bind/metadata_server_test_bind_library/cpp/bind.h>
#include <ddktl/metadata_server.h>

namespace ddk::test {

zx_status_t MetadataRetrieverTestDevice::Bind(void* ctx, zx_device_t* parent) {
  auto retriever = std::make_unique<MetadataRetrieverTestDevice>(parent);

  zx_status_t status = retriever->DdkAdd(ddk::DeviceAddArgs("metadata_retriever"));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add device: %s", zx_status_get_string(status));
    return status;
  }

  [[maybe_unused]] auto unused = retriever.release();
  return ZX_OK;
}

void MetadataRetrieverTestDevice::DdkRelease() { delete this; }

void MetadataRetrieverTestDevice::GetMetadata(GetMetadataCompleter::Sync& completer) {
  zx::result natural = ddk::GetMetadata<fuchsia_hardware_test::Metadata>(parent_);
  if (natural.is_error()) {
    zxlogf(ERROR, "Failed to get metadata: %s", natural.status_string());
    completer.ReplyError(natural.status_value());
    return;
  }

  fidl::Arena arena;
  auto wire = fidl::ToWire(arena, natural.value());

  completer.ReplySuccess(wire);
}

void MetadataRetrieverTestDevice::GetMetadataIfExists(
    GetMetadataIfExistsCompleter::Sync& completer) {
  zx::result result = ddk::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(parent_);
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to get metadata: %s", result.status_string());
    completer.ReplyError(result.status_value());
    return;
  }

  fidl::Arena arena;
  std::optional natural = result.value();
  if (!natural.has_value()) {
    auto empty = fuchsia_hardware_test::wire::Metadata::Builder(arena).Build();
    completer.ReplySuccess(empty, false);
    return;
  }

  auto wire = fidl::ToWire(arena, natural.value());

  completer.ReplySuccess(wire, true);
}

static constexpr zx_driver_ops_t metadata_retriever_test_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = &MetadataRetrieverTestDevice::Bind;
  return ops;
}();

}  // namespace ddk::test

ZIRCON_DRIVER(metadata_retriever, ddk::test::metadata_retriever_test_driver_ops, "zircon", "0.1");
