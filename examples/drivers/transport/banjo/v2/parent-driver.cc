// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/transport/banjo/v2/parent-driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <bind/gizmo/example/cpp/bind.h>

namespace banjo_transport {

zx::result<> ParentBanjoTransportDriver::Start(fdf::DriverContext context) {
  auto child_name = "transport-parent";
  auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  // Initialize our compat server with a banjo config from |banjo_server_|.
  {
    zx::result<> result =
        child_.Initialize(incoming_ptr, outgoing(), context.node_name(), child_name,
                          compat::ForwardMetadata::None(), get_banjo_config());
    if (result.is_error()) {
      return result.take_error();
    }
  }

  // Add a child device node and offer the service capabilities.
  // Offer `fuchsia.examples.gizmo.Service` to the driver that binds to the node.
  zx::result child_result =
      AddChild(child_name,
               {{banjo_server_.property(),
                 fdf::MakeProperty2(bind_gizmo_example::TEST_NODE_ID, "banjo_child")}},
               child_.CreateOffers2());
  if (child_result.is_error()) {
    return child_result.take_error();
  }

  controller_.Bind(std::move(child_result.value()), dispatcher());
  return zx::ok();
}

zx_status_t ParentBanjoTransportDriver::MiscGetHardwareId(uint32_t* out_response) {
  *out_response = 0x1234ABCD;
  return ZX_OK;
}

zx_status_t ParentBanjoTransportDriver::MiscGetFirmwareVersion(uint32_t* out_major,
                                                               uint32_t* out_minor) {
  *out_major = 0x0;
  *out_minor = 0x1;
  return ZX_OK;
}

}  // namespace banjo_transport

FUCHSIA_DRIVER_EXPORT2(banjo_transport::ParentBanjoTransportDriver);
