// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_TRANSPORT_ZIRCON_V2_CHILD_DRIVER_H_
#define EXAMPLES_DRIVERS_TRANSPORT_ZIRCON_V2_CHILD_DRIVER_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>

namespace zircon_transport {

// A driver that binds to the child node added by ParentZirconTransportDriver. This driver
// connects to the fuchsia.hardware.i2c and use it to interact with the parent
class ChildZirconTransportDriver : public fdf::DriverBase2 {
 public:
  ChildZirconTransportDriver() : DriverBase2("zircon-transport-child") {}

  zx::result<> Start(fdf::DriverContext context) override;

  std::string name() const { return name_; }

 private:
  zx::result<> QueryParent(fidl::ClientEnd<fuchsia_hardware_i2c::Device> client_end);

  std::string name_;
  std::vector<std::vector<uint8_t>> read_result_;

  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
};

}  // namespace zircon_transport

#endif  // EXAMPLES_DRIVERS_TRANSPORT_ZIRCON_V2_CHILD_DRIVER_H_
