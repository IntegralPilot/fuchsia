// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_TRANSPORT_ZIRCON_V2_PARENT_DRIVER_H_
#define EXAMPLES_DRIVERS_TRANSPORT_ZIRCON_V2_PARENT_DRIVER_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>

namespace zircon_transport {

// A driver that implements and serves the fuchsia.hardware.i2c FIDL protocol.
class ParentZirconTransportDriver : public fdf::DriverBase2,
                                    public fidl::WireServer<fuchsia_hardware_i2c::Device> {
 public:
  ParentZirconTransportDriver() : DriverBase2("transport-parent") {}

  zx::result<> Start(fdf::DriverContext context) override;

  void Transfer(TransferRequestView request, TransferCompleter::Sync& completer) override;
  void GetName(GetNameCompleter::Sync& completer) override;

 private:
  // Read buffers for transfer requests.
  std::vector<fidl::VectorView<uint8_t>> read_vectors_;
  std::vector<uint8_t> read_buffer_;

  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;

  fidl::ServerBindingGroup<fuchsia_hardware_i2c::Device> bindings_;
};

}  // namespace zircon_transport

#endif  // EXAMPLES_DRIVERS_TRANSPORT_ZIRCON_V2_PARENT_DRIVER_H_
