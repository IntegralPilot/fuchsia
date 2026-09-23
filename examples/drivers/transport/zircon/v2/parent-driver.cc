// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/transport/zircon/v2/parent-driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

namespace zircon_transport {

zx::result<> ParentZirconTransportDriver::Start(fdf::DriverContext context) {
  fuchsia_hardware_i2c::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                        fidl::kIgnoreBindingClosure),
  });
  auto result = outgoing()->AddService<fuchsia_hardware_i2c::Service>(std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add protocol: {}", result);
    return result.take_error();
  }

  // Add a child with a `fuchsia.hardware.i2c.Service` offer.
  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  zx::result child_result =
      AddChild(name(), properties, std::array{fdf::MakeOffer2<fuchsia_hardware_i2c::Service>()});
  if (child_result.is_error()) {
    return child_result.take_error();
  }
  controller_.Bind(std::move(child_result.value()), dispatcher());

  // Populate the read buffers for transfer requests.
  read_buffer_ = {0x1, 0x2, 0x3};
  read_vectors_.emplace_back(fidl::VectorView<uint8_t>::FromExternal(read_buffer_));

  return zx::ok();
}

void ParentZirconTransportDriver::Transfer(TransferRequestView request,
                                           TransferCompleter::Sync& completer) {
  completer.ReplySuccess(fidl::VectorView<fidl::VectorView<uint8_t>>::FromExternal(read_vectors_));
}

void ParentZirconTransportDriver::GetName(GetNameCompleter::Sync& completer) {
  completer.ReplySuccess("i2c_example");
}

}  // namespace zircon_transport

FUCHSIA_DRIVER_EXPORT2(zircon_transport::ParentZirconTransportDriver);
