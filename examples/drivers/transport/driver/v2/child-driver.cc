// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/transport/driver/v2/child-driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace driver_transport {

zx::result<> ChildTransportDriver::Start(fdf::DriverContext context) {
  auto connect_result = context.incoming().Connect<fuchsia_hardware_i2cimpl::Service::Device>();
  if (connect_result.is_error()) {
    fdf::error("Failed to connect fuchsia.hardware.i2cimpl device protocol: {}", connect_result);
    return connect_result.take_error();
  }

  i2c_impl_client_.Bind(std::move(connect_result.value()), driver_dispatcher()->get());

  auto result = QueryInfo();
  if (result.is_error()) {
    return result.take_error();
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  zx::result child_result = AddChild("transport-child", properties, {});
  if (child_result.is_error()) {
    return child_result.take_error();
  }

  controller_.Bind(std::move(child_result.value()), dispatcher());

  // Since we set the dispatcher to "ALLOW_SYNC_CALLS" in the driver CML, we
  // need to seal the option after we finish all our sync calls over driver transport.
  auto status =
      fdf_dispatcher_seal(driver_dispatcher()->get(), FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS);
  if (status != ZX_OK) {
    fdf::error("Failed to seal ALLOW_SYNC_CALLS: {}", zx::make_result(status));
    return zx::error(status);
  }

  return zx::ok();
}

void ChildTransportDriver::SetBitrate(uint32_t bitrate) {
  // Since we sealed synchronous calls at the end of the Start() function, all future calls
  // over the driver transport must be asynchronous.
  fdf::Arena arena('I2CI');
  i2c_impl_client_.buffer(arena)->SetBitrate(bitrate).Then(
      [bitrate](fdf::WireUnownedResult<fuchsia_hardware_i2cimpl::Device::SetBitrate>& result) {
        if (!result.ok()) {
          fdf::error("Failed to set the bitrate: {}", result.error());
        }
        if (result->is_error()) {
          fdf::error("Bitrate request returned an error: {}", result->error_value());
        }
        fdf::info("Successfully set the bitrate to {}", bitrate);
      });
}

zx::result<> ChildTransportDriver::QueryInfo() {
  // Query and store the max transfer size.
  fdf::Arena arena('I2CI');
  auto max_transfer_sz_result = i2c_impl_client_.sync().buffer(arena)->GetMaxTransferSize();
  if (!max_transfer_sz_result.ok()) {
    fdf::error("Failed to request max transfer size: {}", max_transfer_sz_result.error());
    return zx::error(max_transfer_sz_result.status());
  }
  if (max_transfer_sz_result->is_error()) {
    fdf::error("GetMaxTransferSize request returned an error: {}",
               max_transfer_sz_result->error_value());
    return max_transfer_sz_result->take_error();
  }

  max_transfer_size_ = max_transfer_sz_result.value()->size;
  fdf::info("Max transfer size: {}", max_transfer_size_);
  return zx::ok();
}

}  // namespace driver_transport

FUCHSIA_DRIVER_EXPORT2(driver_transport::ChildTransportDriver);
