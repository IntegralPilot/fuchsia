// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/transport/banjo/v2/child-driver.h"

#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace banjo_transport {

zx::result<> ChildBanjoTransportDriver::Start(fdf::DriverContext context) {
  // Connect to the `fuchsia.examples.gizmo.Misc` protocol provided by the parent.
  auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result<ddk::MiscProtocolClient> client =
      compat::ConnectBanjo<ddk::MiscProtocolClient>(incoming_ptr);

  // Since we set the dispatcher to "ALLOW_SYNC_CALLS" in the driver CML, we
  // need to seal the option after we finish all our sync calls.
  zx_status_t status =
      fdf_dispatcher_seal(driver_dispatcher()->get(), FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS);
  if (status != ZX_OK) {
    fdf::error("Failed to seal ALLOW_SYNC_CALLS: {}", zx::make_result(status));
    return zx::error(status);
  }

  if (client.is_error()) {
    fdf::error("Failed to connect client: {}", client);
    return client.take_error();
  }
  client_ = *client;

  status = QueryParent();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  zx::result child_result = AddChild("transport-child", properties, {});
  if (child_result.is_error()) {
    return child_result.take_error();
  }

  controller_.Bind(std::move(child_result.value()), dispatcher());

  return zx::ok();
}

zx_status_t ChildBanjoTransportDriver::QueryParent() {
  zx_status_t status = client_.GetHardwareId(&hardware_id_);
  if (status != ZX_OK) {
    return status;
  }
  fdf::info("Transport client hardware: {:X}", hardware_id_);

  status = client_.GetFirmwareVersion(&major_version_, &minor_version_);
  if (status != ZX_OK) {
    return status;
  }
  fdf::info("Transport client firmware: {}.{}", major_version_, minor_version_);
  return ZX_OK;
}

}  // namespace banjo_transport

FUCHSIA_DRIVER_EXPORT2(banjo_transport::ChildBanjoTransportDriver);
