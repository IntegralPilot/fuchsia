// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.light/cpp/fidl.h>
#include <fidl/fuchsia.hardware.lightsensor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/s905d2/cpp/bind.h>
#include <bind/fuchsia/ams/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/pwm/cpp/bind.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-pwm.h>

#include "astro-gpios.h"
#include "astro.h"

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

zx_status_t Astro::LightInit() {
  // TODO(kpt): Insert the right parameters here.
  static const fuchsia_hardware_lightsensor::Metadata kLightSensorMetadata({
      .gain = 64,
      .integration_time = zx::usec(711'680).get(),
      .polling_time = zx::usec(700'000).get(),
  });

  fit::result persisted_light_sensor_metadata = fidl::Persist(kLightSensorMetadata);
  if (!persisted_light_sensor_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist light sensor metadata: %s",
           persisted_light_sensor_metadata.error_value().FormatDescription().c_str());
    return persisted_light_sensor_metadata.error_value().status();
  }

  const fpbus::Node tcs3400_light_node({
      .name = "tcs3400-light-39",
      .vid = bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC,
      .pid = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC,
      .did = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_TCS3400_LIGHT,
      .metadata =
          std::vector<fpbus::Metadata>{
              {{
                  .id = fuchsia_hardware_lightsensor::Metadata::kSerializableName,
                  .data = std::move(persisted_light_sensor_metadata.value()),
              }},
          },
  });

  const auto kI2cBindRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "als"),
  };
  const auto kI2cProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
  };

  const auto kGpioLightInterruptRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d2::GPIOAO_PIN_ID_PIN_5),
  };
  const auto kGpioLightInterruptProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-int"),
  };

  auto kTcs3400LightParents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kI2cBindRules,
          .properties = kI2cProperties,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kGpioLightInterruptRules,
          .properties = kGpioLightInterruptProperties,
      }},
  };

  fidl::Arena<> fidl_arena;
  fdf::Arena tcs3400_light_arena('TCS3');

  auto tcs3400_light_spec = fuchsia_driver_framework::CompositeNodeSpec{
      {.name = "tcs3400-light-39", .parents2 = kTcs3400LightParents}};
  fdf::WireUnownedResult tsc3400_light_result =
      pbus_.buffer(tcs3400_light_arena)
          ->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, tcs3400_light_node),
                                 fidl::ToWire(fidl_arena, tcs3400_light_spec));
  if (!tsc3400_light_result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request to platform bus: %s",
           tsc3400_light_result.status_string());
    return tsc3400_light_result.status();
  }
  if (tsc3400_light_result->is_error()) {
    zxlogf(ERROR, "Failed to add tcs3400_light composite node spec to platform device: %s",
           zx_status_get_string(tsc3400_light_result->error_value()));
    return tsc3400_light_result->error_value();
  }

  // Lights
  // Instructions: include fragments in this order
  //     GPIO fragment
  //     BRIGHTNESS capable--include PWM fragment
  //     RGB capable--include RGB fragment
  //   Set GPIO alternative function here!
  static const std::vector<fuchsia_hardware_light::Config> kConfigs{
      {{.name = "AMBER_LED", .brightness = true, .rgb = false, .init_on = true, .group_id = -1}}};
  static const fuchsia_hardware_light::Metadata kMetadata{{.configs = kConfigs}};

  auto metadata = fidl::Persist(kMetadata);
  if (!metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist metadata: %s",
           metadata.error_value().FormatDescription().c_str());
    return metadata.error_value().status();
  }

  fpbus::Node light_node;
  light_node.name() = "aml-light";
  light_node.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  light_node.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
  light_node.did() = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_GPIO_LIGHT;
  light_node.metadata() = {
      {{
          .id = fuchsia_hardware_light::Metadata::kSerializableName,
          .data = std::move(metadata.value()),
      }},
  };

  // Enable the Amber LED so it will be controlled by PWM.
  gpio_init_steps_.push_back(GpioFunction(GPIO_AMBER_LED, 3));  // Set as PWM.

  // GPIO must be set to default out otherwise could cause light to not work
  // on certain reboots.
  gpio_init_steps_.push_back(GpioOutput(GPIO_AMBER_LED, true));

  auto amber_led_gpio_bind_rules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d2::GPIOAO_PIN_ID_PIN_11),
  };

  auto amber_led_gpio_properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-amber-led"),
  };

  auto amber_led_pwm_bind_rules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID, bind_fuchsia_amlogic_platform_s905d2::ID_PWM_AO_A),
  };

  auto amber_led_pwm_properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "pwm-amber-led"),
  };
  const auto kGpioInitBindRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };
  const auto kGpioInitProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };

  auto aml_light_parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = amber_led_gpio_bind_rules,
          .properties = amber_led_gpio_properties,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = amber_led_pwm_bind_rules,
          .properties = amber_led_pwm_properties,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kGpioInitBindRules,
          .properties = kGpioInitProperties,
      }},
  };

  fdf::Arena arena('LIGH');

  auto aml_light_spec = fuchsia_driver_framework::CompositeNodeSpec{
      {.name = "aml-light", .parents2 = aml_light_parents}};
  fdf::WireUnownedResult result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, light_node), fidl::ToWire(fidl_arena, aml_light_spec));

  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Light(aml_light) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Light(aml_light) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace astro
