// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_AML_UART_AML_UART_DFV2_H_
#define SRC_DEVICES_SERIAL_DRIVERS_AML_UART_AML_UART_DFV2_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>

#include "src/devices/serial/drivers/aml-uart/aml-uart.h"
#include "src/devices/serial/drivers/aml-uart/aml_uart_config.h"

namespace serial {

class AmlUartV2 : public fdf::DriverBase2 {
 public:
  explicit AmlUartV2() : fdf::DriverBase2("aml-uart") {}

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;

  // Used by the unit test to access the device.
  AmlUart& aml_uart_for_testing();

 private:
  fuchsia_hardware_serial::wire::SerialPortInfo serial_port_info_;
  std::optional<AmlUart> aml_uart_;
  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> serial_impl_bindings_;

  aml_uart_config::Config driver_config_;

  fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>
      mac_address_metadata_server_;
};

}  // namespace serial

#endif  // SRC_DEVICES_SERIAL_DRIVERS_AML_UART_AML_UART_DFV2_H_
