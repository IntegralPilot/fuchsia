// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/metadata/cpp/tests/metadata_integration_test/metadata_retriever_test_driver/metadata_retriever_test_driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/driver/metadata/cpp/metadata.h>

namespace fdf_metadata::test {

zx::result<> MetadataRetrieverTestDriver::Start(fdf::DriverContext context) {
  incoming_ = context.take_incoming();
  zx::result result = outgoing()->AddService<fuchsia_hardware_test::MetadataRetrieverService>(
      fuchsia_hardware_test::MetadataRetrieverService::InstanceHandler(
          {.device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure)}));
  if (result.is_error()) {
    fdf::error("Failed to add service: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

void MetadataRetrieverTestDriver::GetMetadata(GetMetadataCompleter::Sync& completer) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  zx::result metadata = fdf_metadata::GetMetadata<fuchsia_hardware_test::Metadata>(*incoming_);

  if (metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata);
    completer.Reply(fit::error(metadata.status_value()));
    return;
  }

  completer.Reply(fit::ok(std::move(metadata.value())));
#else
  fdf::error("Getting metadata not supported at current Fuchsia API level.");
  completer.Reply(fit::error(ZX_ERR_UNSUPPORTED));
#endif
}

void MetadataRetrieverTestDriver::GetMetadataIfExists(
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    GetMetadataIfExistsCompleter::Sync& completer) {
  zx::result result =
      fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(*incoming_);
  if (result.is_error()) {
    fdf::error("Failed to get metadata: {}", result);
    completer.Reply(fit::error(result.status_value()));
    return;
  }

  std::optional metadata = std::move(result.value());
  if (!metadata.has_value()) {
    fuchsia_hardware_test::MetadataRetrieverGetMetadataIfExistsResponse response{
        {.metadata = {}, .retrieved_metadata = false}};
    completer.Reply(fit::ok(std::move(response)));
    return;
  }

  fuchsia_hardware_test::MetadataRetrieverGetMetadataIfExistsResponse response{
      {.metadata = std::move(metadata.value()), .retrieved_metadata = true}};
  completer.Reply(fit::ok(std::move(response)));
#else
  fdf::error("Getting metadata not supported at current Fuchsia API level.");
  completer.Reply(fit::error(ZX_ERR_UNSUPPORTED));
#endif
}

}  // namespace fdf_metadata::test

FUCHSIA_DRIVER_EXPORT2(fdf_metadata::test::MetadataRetrieverTestDriver);
