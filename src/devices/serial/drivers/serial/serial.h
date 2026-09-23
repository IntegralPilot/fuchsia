// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_SERIAL_SERIAL_H_
#define SRC_DEVICES_SERIAL_DRIVERS_SERIAL_SERIAL_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.serial/cpp/wire.h>
#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <zircon/types.h>

#include "src/devices/serial/drivers/serial/serial_config.h"

namespace serial {

class SerialDevice : public fdf::DriverBase2,
                     public fidl::WireServer<fuchsia_hardware_serial::DeviceProxy>,
                     public fidl::WireServer<fuchsia_hardware_serial::Device> {
 public:
  explicit SerialDevice()
      : fdf::DriverBase2("serial"),
        devfs_connector_(fit::bind_member<&SerialDevice::DevfsConnect>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  zx_status_t Bind(serial_config::Config config);
  zx_status_t Init();

  void GetChannel(GetChannelRequestView request, GetChannelCompleter::Sync& completer) override;

  void Read(ReadCompleter::Sync& completer) override;
  void Write(WriteRequestView request, WriteCompleter::Sync& completer) override;

 protected:
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }

 private:
  // Fidl protocol implementation.
  void GetClass(GetClassCompleter::Sync& completer) override;
  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override;

  zx_status_t Enable(bool enable);
  void ResetSerialImplConnectionAndThen(fit::closure completer);

  zx_status_t Bind(fidl::ServerEnd<fuchsia_hardware_serial::Device> server);
  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_serial::DeviceProxy> server);

  // The serial protocol of the device we are binding against.
  fdf::WireClient<fuchsia_hardware_serialimpl::Device> serial_;

  uint32_t serial_class_;
  driver_devfs::Connector<fuchsia_hardware_serial::DeviceProxy> devfs_connector_;

  std::shared_ptr<fdf::Namespace> incoming_;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> controller_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> impl_controller_;

  fidl::ServerBindingGroup<fuchsia_hardware_serial::DeviceProxy> proxy_bindings_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_serial::Device>> binding_;

  fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>
      mac_address_metadata_server_;
};

}  // namespace serial

#endif  // SRC_DEVICES_SERIAL_DRIVERS_SERIAL_SERIAL_H_
