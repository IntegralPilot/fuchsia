// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/ot-radio/ot-radio.h>
#include <limits.h>
#include <unistd.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/nordic/platform/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <fbl/algorithm.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock-gpios.h"
#include "sherlock.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

const std::vector<fuchsia_driver_framework::BindRule2> kSpiRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                            bind_fuchsia_nordic_platform::BIND_PLATFORM_DEV_VID_NORDIC),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                            bind_fuchsia_nordic_platform::BIND_PLATFORM_DEV_PID_NRF52840),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                            bind_fuchsia_nordic_platform::BIND_PLATFORM_DEV_DID_THREAD),

};

const std::vector<fuchsia_driver_framework::NodeProperty2> kSpiProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
    fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                       bind_fuchsia_nordic_platform::BIND_PLATFORM_DEV_VID_NORDIC),
    fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                       bind_fuchsia_nordic_platform::BIND_PLATFORM_DEV_DID_THREAD),
};

const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const std::map<uint32_t, std::string> kGpioPinFunctionMap = {
    {GPIO_OT_RADIO_INTERRUPT, "gpio-int"},
    {GPIO_OT_RADIO_RESET, "gpio-reset"},
    {GPIO_OT_RADIO_BOOTLOADER, "gpio-bootloader"},
};

zx_status_t Sherlock::OtRadioInit() {
  gpio_init_steps_.push_back(GpioPull(GPIO_OT_RADIO_INTERRUPT, fuchsia_hardware_pin::Pull::kNone));

  fpbus::Node dev;
  dev.name() = "nrf52840-radio";
  dev.vid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC;
  dev.pid() = bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_SHERLOCK;
  dev.did() = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_OT_RADIO;

  std::vector<fuchsia_driver_framework::ParentSpec2> parents = {
      fuchsia_driver_framework::ParentSpec2{{kSpiRules, kSpiProperties}},
      fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProperties}},
  };
  parents.reserve(parents.size() + kGpioPinFunctionMap.size());

  for (auto& [gpio_pin, function] : kGpioPinFunctionMap) {
    auto rules = std::vector{
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, gpio_pin),
    };
    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, function),
    };
    parents.push_back(fuchsia_driver_framework::ParentSpec2{{rules, properties}});
  }

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('RDIO');
  fdf::WireUnownedResult result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, dev),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "ot-radio-0", .parents2 = parents}}));

  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request to platform bus: %s",
           result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add nrf52840-radio composite to platform device: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace sherlock
