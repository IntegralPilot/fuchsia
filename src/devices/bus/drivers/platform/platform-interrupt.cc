// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bus/drivers/platform/platform-interrupt.h"

#include <fidl/fuchsia.hardware.interrupt/cpp/markers.h>

#include <iterator>

#include <bind/fuchsia/cpp/bind.h>

#include "src/devices/bus/drivers/platform/platform-bus.h"
#include "src/devices/bus/drivers/platform/platform-device.h"

namespace platform_bus {

void PlatformInterruptFragment::Get(GetCompleter::Sync& completer) {
  pdev_->GetInterrupt(index_, 0, PlatformDevice::MakeGetInterruptCallback(completer));
}

zx::result<> PlatformInterruptFragment::Add(std::string_view name, PlatformDevice* pdev,
                                            fuchsia_hardware_platform_bus::Irq& irq) {
  fuchsia_hardware_interrupt::Service::InstanceHandler handler(
      {.provider = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure)});

  zx::result result = pdev->bus()->outgoing()->AddService<fuchsia_hardware_interrupt::Service>(
      std::move(handler), name);
  if (result.is_error()) {
    return result.take_error();
  }

  std::array offers = {
      fdf::MakeOffer2<fuchsia_hardware_interrupt::Service>(name),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> props;

  if (const auto& irq_props = irq.properties(); irq_props.has_value()) {
    std::copy(irq_props->cbegin(), irq_props->cend(), std::back_inserter(props));
  } else {
    props = {
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID, pdev->vid()),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID, pdev->did()),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID, pdev->pid()),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, pdev->instance_id()),
        // Because "x == 0" is true if "x" is unset, start at 1.
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_INTERRUPT_ID, index_ + 1),
    };
  }

  zx::result child =
      fdf::AddChild(pdev->bus()->platform_node(), pdev->bus()->logger(), name, props, offers);
  if (child.is_error()) {
    return child.take_error();
  }
  node_ = std::move(child.value());
  return zx::ok();
}

}  // namespace platform_bus
