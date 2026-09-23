// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_AML_ETHERNET_AML_ETHERNET_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_AML_ETHERNET_AML_ETHERNET_H_

#include <fidl/fuchsia.hardware.ethernet.board/cpp/wire.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/device.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <threads.h>

#include <optional>

#include <ddktl/device.h>
#include <fbl/macros.h>

#include "src/devices/i2c/lib/i2c-channel-legacy/i2c-channel.h"

namespace eth {

class AmlEthernet;
using DeviceType = ddk::Device<AmlEthernet>;

class AmlEthernet : public DeviceType,
                    public fidl::WireServer<fuchsia_hardware_ethernet_board::EthBoard> {
 public:
  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(AmlEthernet);

  explicit AmlEthernet(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Create(void* ctx, zx_device_t* parent);

  // DDK Hooks.
  void DdkRelease();

  // ETH_BOARD protocol.
  void ResetPhy(ResetPhyCompleter::Sync& completer);

 private:
  // GPIO Indexes.
  enum {
    PHY_RESET,
    PHY_INTR,
    GPIO_COUNT,
  };

  // MMIO Indexes
  enum {
    MMIO_PERIPH,
    MMIO_HHI,
  };

  zx_status_t InitPdev();
  zx_status_t Bind();

  fdf::PDev pdev_;
  ddk::I2cChannel i2c_;
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> gpios_[GPIO_COUNT];
  std::optional<component::OutgoingDirectory> outgoing_;
  fidl::ServerEnd<fuchsia_io::Directory> outgoing_server_end_;
  fidl::ServerBindingGroup<fuchsia_hardware_ethernet_board::EthBoard> bindings_;

  std::optional<fdf::MmioBuffer> periph_mmio_;
  std::optional<fdf::MmioBuffer> hhi_mmio_;
};

}  // namespace eth

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_AML_ETHERNET_AML_ETHERNET_H_
