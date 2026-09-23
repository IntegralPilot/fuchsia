// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/metadata/cpp/tests/metadata_integration_test/test_root/test_root.h"

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <bind/fuchsia_driver_metadata_test_bind_library/cpp/bind.h>

namespace fdf_metadata::test {

zx::result<> TestRootDriver::Start(fdf::DriverContext context) {
  zx::result result = outgoing()->AddService<fuchsia_hardware_test::RootService>(
      fuchsia_hardware_test::RootService::InstanceHandler(
          {.device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure)}));
  if (result.is_error()) {
    fdf::error("Failed to add service: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

void TestRootDriver::AddMetadataSenderNode(AddMetadataSenderNodeRequest& request,
                                           AddMetadataSenderNodeCompleter::Sync& completer) {
  bool exposes_metadata_fidl_service = request.exposes_metadata_fidl_service();

  std::vector<fuchsia_driver_framework::NodeProperty2> node_properties{
      fdf::MakeProperty2(bind_fuchsia_driver_metadata_test::PURPOSE,
                         bind_fuchsia_driver_metadata_test::PURPOSE_SEND_METADATA),
      fdf::MakeProperty2(bind_fuchsia_driver_metadata_test::EXPOSES_METADATA_FIDL_SERVICE,
                         exposes_metadata_fidl_service)};

  const std::string node_name = std::format(
      "{}-{}", exposes_metadata_fidl_service ? "expose" : "no_expose", metadata_senders_.size());
  zx::result result = AddChild(node_name, node_properties, {});
  if (result.is_error()) {
    fdf::error("Failed to add child: {}", result);
    completer.Reply(fit::error(result.status_value()));
    return;
  }

  metadata_senders_.emplace_back(std::move(result.value()));
  completer.Reply(fit::ok());
}

}  // namespace fdf_metadata::test

FUCHSIA_DRIVER_EXPORT2(fdf_metadata::test::TestRootDriver);
