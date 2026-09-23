// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_INTERRUPT_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_INTERRUPT_H_

#include <fidl/fuchsia.hardware.interrupt/cpp/wire_messaging.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

namespace platform_bus {

class PlatformDevice;

class PlatformInterruptFragment : public fidl::WireServer<fuchsia_hardware_interrupt::Provider> {
 public:
  PlatformInterruptFragment(PlatformDevice* pdev, uint32_t index, async_dispatcher_t* dispatcher)
      : pdev_(pdev), index_(index), dispatcher_(dispatcher) {}

  // Interrupt provider implementation.
  void Get(GetCompleter::Sync& completer) override;

  zx::result<> Add(std::string_view name, PlatformDevice* pdev,
                   fuchsia_hardware_platform_bus::Irq& irq);

 private:
  PlatformDevice* pdev_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> node_;
  uint32_t index_;
  fidl::ServerBindingGroup<fuchsia_hardware_interrupt::Provider> bindings_;
  async_dispatcher_t* dispatcher_;
};

}  // namespace platform_bus

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_INTERRUPT_H_
