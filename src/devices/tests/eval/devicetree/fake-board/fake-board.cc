// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/eval/devicetree/fake-board/fake-board.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/devicetree/manager/publisher-dev.h>
#include <lib/driver/devicetree/visitors/load-visitors.h>
#include <lib/driver/logging/cpp/logger.h>

namespace devicetree_evaluation {

zx::result<> FakeBoard::Start(fdf::DriverContext context) {
  node_.Bind(take_node());

  auto manager = fdf_devicetree::Manager::CreateFromNamespace(context.incoming());
  if (manager.is_error()) {
    FDF_LOG(ERROR, "Failed to create devicetree manager: %s", manager.status_string());
    return manager.take_error();
  }

  auto visitors = fdf_devicetree::LoadVisitors(
      context.symbols().value_or(std::vector<fuchsia_driver_framework::NodeSymbol>{}));
  if (visitors.is_error()) {
    FDF_LOG(ERROR, "Failed to create visitors: %s", visitors.status_string());
    return visitors.take_error();
  }

  auto status = manager->Walk(*(visitors.value()));
  if (status.is_error()) {
    FDF_LOG(ERROR, "Failed to walk the device tree: %s", status.status_string());
    return status.take_error();
  }

  auto pbus = context.incoming().Connect<fuchsia_hardware_platform_bus::Service::PlatformBus>();
  if (pbus.is_error() || !pbus->is_valid()) {
    FDF_LOG(ERROR, "Failed to connect to pbus: %s", pbus.status_string());
    return pbus.take_error();
  }

  auto group_manager = context.incoming().Connect<fuchsia_driver_framework::CompositeNodeManager>();
  if (group_manager.is_error()) {
    FDF_LOG(ERROR, "Failed to connect to device group manager: %s", group_manager.status_string());
    return group_manager.take_error();
  }

  auto pbus_client = fdf::WireSyncClient(std::move(pbus.value()));
  auto mgr_client = fidl::SyncClient(std::move(group_manager.value()));
  fdf_devicetree::PublisherDev publisher(pbus_client, mgr_client, node_);
  status = manager->PublishDevices(publisher);
  if (status.is_error()) {
    FDF_LOG(ERROR, "Failed to publish devices: %s", status.status_string());
    return status.take_error();
  }

  return zx::ok();
}

}  // namespace devicetree_evaluation

FUCHSIA_DRIVER_EXPORT2(devicetree_evaluation::FakeBoard);
