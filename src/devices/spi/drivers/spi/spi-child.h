// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SPI_DRIVERS_SPI_SPI_CHILD_H_
#define SRC_DEVICES_SPI_DRIVERS_SPI_SPI_CHILD_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spi/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spiimpl/cpp/driver/fidl.h>
#include <lib/driver/devfs/cpp/connector.h>

namespace spi {

class SpiDriver;

class SpiChild : public fidl::WireServer<fuchsia_hardware_spi::Device>,
                 public fidl::WireServer<fuchsia_hardware_spi::Controller> {
 public:
  SpiChild(fdf::WireSharedClient<fuchsia_hardware_spiimpl::SpiImpl> spi, uint32_t chip_select,
           bool has_siblings, fdf::UnownedSynchronizedDispatcher fidl_dispatcher)
      : spi_(std::move(spi)),
        cs_(chip_select),
        has_siblings_(has_siblings),
        fidl_dispatcher_(std::move(fidl_dispatcher)),
        devfs_connector_(fit::bind_member<&SpiChild::DevfsConnect>(this)) {}

  void OpenSession(OpenSessionRequestView request, OpenSessionCompleter::Sync& completer) override;

  void TransmitVector(TransmitVectorRequestView request,
                      TransmitVectorCompleter::Sync& completer) override;
  void ReceiveVector(ReceiveVectorRequestView request,
                     ReceiveVectorCompleter::Sync& completer) override;
  void ExchangeVector(ExchangeVectorRequestView request,
                      ExchangeVectorCompleter::Sync& completer) override;

  void RegisterVmo(RegisterVmoRequestView request, RegisterVmoCompleter::Sync& completer) override;
  void UnregisterVmo(UnregisterVmoRequestView request,
                     UnregisterVmoCompleter::Sync& completer) override;

  void Transmit(TransmitRequestView request, TransmitCompleter::Sync& completer) override;
  void Receive(ReceiveRequestView request, ReceiveCompleter::Sync& completer) override;
  void Exchange(ExchangeRequestView request, ExchangeCompleter::Sync& completer) override;

  void CanAssertCs(CanAssertCsCompleter::Sync& completer) override;
  void AssertCs(AssertCsCompleter::Sync& completer) override;
  void DeassertCs(DeassertCsCompleter::Sync& completer) override;

  void Bind(fidl::ServerEnd<fuchsia_hardware_spi::Device> server_end);

  fuchsia_hardware_spi::Service::InstanceHandler CreateInstanceHandler();

  zx::result<fidl::ClientEnd<fuchsia_device_fs::Connector>> BindDevfs() {
    return devfs_connector_.Bind(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

 private:
  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_spi::Controller> server);

  fdf::WireSharedClient<fuchsia_hardware_spiimpl::SpiImpl> spi_;
  const uint32_t cs_;
  // False if this child is the only device on the bus.
  const bool has_siblings_;
  const fdf::UnownedSynchronizedDispatcher fidl_dispatcher_;

  std::optional<fidl::ServerBinding<fuchsia_hardware_spi::Device>> binding_;
  fidl::ServerBindingGroup<fuchsia_hardware_spi::Controller> controller_bindings_;
  driver_devfs::Connector<fuchsia_hardware_spi::Controller> devfs_connector_;
};

}  // namespace spi

#endif  // SRC_DEVICES_SPI_DRIVERS_SPI_SPI_CHILD_H_
