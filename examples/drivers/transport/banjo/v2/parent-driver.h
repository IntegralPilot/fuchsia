// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_TRANSPORT_BANJO_V2_PARENT_DRIVER_H_
#define EXAMPLES_DRIVERS_TRANSPORT_BANJO_V2_PARENT_DRIVER_H_

#include <fuchsia/examples/gizmo/cpp/banjo.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>

#include <bind/fuchsia/platform/cpp/bind.h>

namespace banjo_transport {

// Parent driver that serves the Misc protocol over Banjo transport.
class ParentBanjoTransportDriver : public fdf::DriverBase2,
                                   public ddk::MiscProtocol<ParentBanjoTransportDriver> {
 public:
  ParentBanjoTransportDriver() : DriverBase2("banjo-transport-parent") {}

  zx::result<> Start(fdf::DriverContext context) override;

  // MiscProtocol implementation.
  zx_status_t MiscGetHardwareId(uint32_t* out_response);
  zx_status_t MiscGetFirmwareVersion(uint32_t* out_major, uint32_t* out_minor);

 private:
  compat::DeviceServer::BanjoConfig get_banjo_config() {
    compat::DeviceServer::BanjoConfig config{bind_fuchsia_platform::BIND_PROTOCOL_MISC};
    config.callbacks[bind_fuchsia_platform::BIND_PROTOCOL_MISC] = banjo_server_.callback();
    return config;
  }

  fidl::Client<fuchsia_driver_framework::NodeController> controller_;

  compat::BanjoServer banjo_server_{bind_fuchsia_platform::BIND_PROTOCOL_MISC, this,
                                    &misc_protocol_ops_};
  compat::SyncInitializedDeviceServer child_;
};

}  // namespace banjo_transport

#endif  // EXAMPLES_DRIVERS_TRANSPORT_BANJO_V2_PARENT_DRIVER_H_
