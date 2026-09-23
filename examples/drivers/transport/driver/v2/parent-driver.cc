// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/transport/driver/v2/parent-driver.h"

#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

namespace driver_transport {

zx::result<> ParentTransportDriver::Start(fdf::DriverContext context) {
  // Publish `fuchsia.hardware.i2cimpl.Service` to the outgoing directory.
  fuchsia_hardware_i2cimpl::Service::InstanceHandler handler({
      .device = server_bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                               fidl::kIgnoreBindingClosure),
  });
  zx::result result = outgoing()->AddService<fuchsia_hardware_i2cimpl::Service>(std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add service: {}", result);
    return result.take_error();
  }

  // Add a child with a `fuchsia.examples.gizmo.Service` offer.
  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  zx::result child_result =
      AddChild("driver_transport_child", properties,
               std::array{fdf::MakeOffer2<fuchsia_hardware_i2cimpl::Service>()});
  if (child_result.is_error()) {
    return child_result.take_error();
  }

  controller_.Bind(std::move(child_result.value()), dispatcher());
  return zx::ok();
}

void ParentTransportDriver::GetMaxTransferSize(fdf::Arena& arena,
                                               GetMaxTransferSizeCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(0x1234ABCD);
}

void ParentTransportDriver::SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                                       SetBitrateCompleter::Sync& completer) {
  bitrate_ = request->bitrate;
  completer.buffer(arena).ReplySuccess();
}

void ParentTransportDriver::Transact(TransactRequestView request, fdf::Arena& arena,
                                     TransactCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_i2cimpl::wire::ReadData> reads;
  reads.push_back({.data = fidl::VectorView<uint8_t>(arena, {0, 1, 2})});
  completer.buffer(arena).ReplySuccess({arena, reads});
}

void ParentTransportDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_i2cimpl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error(
      "Unknown method in fuchsia.hardware.i2cimpl Device protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace driver_transport

FUCHSIA_DRIVER_EXPORT2(driver_transport::ParentTransportDriver);
