// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.buttons/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/t931/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <ddktl/device.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "lib/fidl_driver/cpp/wire_messaging_declarations.h"
#include "sherlock-gpios.h"
#include "src/devices/board/drivers/sherlock/sherlock.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

zx_status_t Sherlock::ButtonsInit() {
  static const fuchsia_buttons::GpioButtonConfig kVolumeUp(
      {.type = fuchsia_buttons::GpioButtonType::WithDirect({}),
       .gpio_a_index = 0,
       .id = fuchsia_buttons::GpioButtonId::kVolumeUp});

  static const fuchsia_buttons::GpioButtonConfig kVolumeDown(
      {.type = fuchsia_buttons::GpioButtonType::WithDirect({}),
       .gpio_a_index = 1,
       .id = fuchsia_buttons::GpioButtonId::kVolumeDown});

  static const fuchsia_buttons::GpioButtonConfig kFdr(
      {.type = fuchsia_buttons::GpioButtonType::WithDirect({}),
       .gpio_a_index = 2,
       .id = fuchsia_buttons::GpioButtonId::kFdr});

  static const fuchsia_buttons::GpioButtonConfig kMicAndCamMute(
      {.type = fuchsia_buttons::GpioButtonType::WithDirect({}),
       .gpio_a_index = 3,
       .id = fuchsia_buttons::GpioButtonId::kMicAndCamMute});

  // No need for internal pull, external pull-ups used.
  static const std::vector<fuchsia_buttons::GpioConfig> kGpioConfigs = {
      {{.type = fuchsia_buttons::GpioType::WithInterrupt({}),
        .flags = fuchsia_buttons::GpioFlag::kInverted}},
      {{.type = fuchsia_buttons::GpioType::WithInterrupt({}),
        .flags = fuchsia_buttons::GpioFlag::kInverted}},
      {{.type = fuchsia_buttons::GpioType::WithInterrupt({}),
        .flags = fuchsia_buttons::GpioFlag::kInverted}},
      {{.type = fuchsia_buttons::GpioType::WithInterrupt({}),
        .flags = fuchsia_buttons::GpioFlag{0}}}};

  static const fuchsia_buttons::GpioButtonsMetadata kMetadata(
      {.buttons = std::vector{kVolumeUp, kVolumeDown, kFdr, kMicAndCamMute},
       .gpios = kGpioConfigs});

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist pin metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  auto button_pin = [](uint32_t pin, fuchsia_hardware_pin::Pull pull) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = pin,
        .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
            .pull = pull,
            .function = 0,
        }}),
    }});
  };

  gpio_init_steps_.push_back(button_pin(GPIO_VOLUME_UP, fuchsia_hardware_pin::Pull::kUp));
  gpio_init_steps_.push_back(button_pin(GPIO_VOLUME_DOWN, fuchsia_hardware_pin::Pull::kUp));
  gpio_init_steps_.push_back(button_pin(GPIO_VOLUME_BOTH, fuchsia_hardware_pin::Pull::kNone));
  gpio_init_steps_.push_back(button_pin(GPIO_MIC_PRIVACY, fuchsia_hardware_pin::Pull::kNone));

  fidl::Arena<> fidl_arena;
  fdf::Arena buttons_arena('BTTN');

  fpbus::Node dev({.name = "gpio-buttons",
                   .vid = PDEV_VID_GENERIC,
                   .pid = PDEV_PID_GENERIC,
                   .did = PDEV_DID_BUTTONS,
                   .metadata = std::vector<fpbus::Metadata>{{{
                       .id = fuchsia_buttons::GpioButtonsMetadata::kSerializableName,
                       .data = std::move(persisted_metadata.value()),
                   }}}});

  const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };
  const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProps = {
      fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> kVolUpRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_t931::GPIOZ_PIN_ID_PIN_4)};
  const std::vector<fuchsia_driver_framework::NodeProperty2> kVolUpProps = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "volume-up"),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> kVolDownRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_t931::GPIOZ_PIN_ID_PIN_5)};
  const std::vector<fuchsia_driver_framework::NodeProperty2> kVolDownProps = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "volume-down"),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> kVolBothRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_t931::GPIOZ_PIN_ID_PIN_13)};
  const std::vector<fuchsia_driver_framework::NodeProperty2> kVolBothProps = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "volume-both"),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> kMicPrivacyRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_t931::GPIOH_PIN_ID_PIN_3)};
  const std::vector<fuchsia_driver_framework::NodeProperty2> kMicPrivacyProps = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "mic-mute"),
  };

  std::vector<fuchsia_driver_framework::ParentSpec2> parents = {
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = std::move(kGpioInitRules),
          .properties = std::move(kGpioInitProps),
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = std::move(kVolUpRules),
          .properties = std::move(kVolUpProps),
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = std::move(kVolDownRules),
          .properties = std::move(kVolDownProps),
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = std::move(kVolBothRules),
          .properties = std::move(kVolBothProps),
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = std::move(kMicPrivacyRules),
          .properties = std::move(kMicPrivacyProps),
      }}};

  fuchsia_driver_framework::CompositeNodeSpec buttonComposite = {
      {.name = "gpio-buttons", .parents2 = std::move(parents)}};

  fdf::WireUnownedResult result =
      pbus_.buffer(buttons_arena)
          ->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, dev),
                                 fidl::ToWire(fidl_arena, buttonComposite));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec error: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace sherlock
