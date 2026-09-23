// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SPI_DRIVERS_SPI_SPI_H_
#define SRC_DEVICES_SPI_DRIVERS_SPI_SPI_H_

#include <fidl/fuchsia.hardware.spi.businfo/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spi/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spiimpl/cpp/driver/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <optional>
#include <vector>

#include "spi-child.h"
#include "src/devices/spi/drivers/spi/spi_config.h"

namespace spi {

class SpiDriver : public fdf::DriverBase2, public fidl::WireServer<fuchsia_hardware_spi::Test> {
 public:
  static constexpr std::string_view kDriverName = "spi";
  static constexpr std::string_view kChildNodeName = "spi";

  SpiDriver() : fdf::DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  zx::result<> AddTestChild(fdf::WireSharedClient<fuchsia_hardware_spiimpl::SpiImpl> client);

  zx::result<> AddChildren(const fuchsia_hardware_spi_businfo::SpiBusMetadata& metadata,
                           fdf::WireSharedClient<fuchsia_hardware_spiimpl::SpiImpl> client,
                           const spi_config::Config& config);

  void ConnectSpiLoopback(fuchsia_hardware_spi::wire::TestConnectSpiLoopbackRequest* request,
                          ConnectSpiLoopbackCompleter::Sync& completer) override;
  void Get(GetCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_spi::Test> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  fdf::UnownedSynchronizedDispatcher fidl_dispatcher() {
    if (fidl_dispatcher_) {
      return fidl_dispatcher_->borrow();
    }
    return fdf::UnownedSynchronizedDispatcher(fdf::Dispatcher::GetCurrent()->get());
  }

  const std::unique_ptr<fdf::Namespace>& incoming() const { return incoming_; }

  uint32_t bus_id_ = 0;
  std::optional<fdf::SynchronizedDispatcher> fidl_dispatcher_;

  std::vector<std::unique_ptr<SpiChild>> children_;

  fdf::OwnedChildNode child_;

  std::unique_ptr<fdf::Namespace> incoming_;
  zx::event node_token_;

  std::unique_ptr<SpiChild> test_child_;
  fidl::ServerBindingGroup<fuchsia_hardware_spi::Test> test_bindings_;
};

}  // namespace spi

#endif  // SRC_DEVICES_SPI_DRIVERS_SPI_SPI_H_
