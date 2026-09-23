// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/s905d3/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/power/cpp/bind.h>
#include <bind/fuchsia/ti/platform/cpp/bind.h>
#include <ddktl/device.h>

#include "nelson-gpios.h"
#include "nelson.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

// These values are specific to Nelson, and are only used within this board driver.
enum : uint32_t {
  kPowerSensorDomainMlb = 0,
  kPowerSensorDomainAudio = 1,
};

zx_status_t AddMlbComposite(fdf::WireSyncClient<fpbus::PlatformBus>& pbus,
                            fidl::AnyArena& fidl_arena, fdf::Arena& arena) {
  static const fuchsia_hardware_ti_metadata::Ina231Metadata kMetadata({
      .mode = fuchsia_hardware_ti_metadata::Mode::kShuntAndBusContinuous,
      .shunt_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .bus_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .averages = fuchsia_hardware_ti_metadata::Averages::kAverages1024,
      .shunt_resistance_microohm = 10'000,
      .bus_voltage_limit_microvolt = 0,
      .alert = fuchsia_hardware_ti_metadata::Alert::kNone,
      .power_sensor_domain = kPowerSensorDomainMlb,
  });

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  fpbus::Node node({
      .name = "ti-ina231-mlb",
      .vid = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI,
      .pid = bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_NELSON,
      .did = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_INA231_MLB,
      .metadata =
          std::vector<fpbus::Metadata>{
              {{
                  .id = fuchsia_hardware_ti_metadata::Ina231Metadata::kSerializableName,
                  .data = std::move(persisted_metadata.value()),
              }},
          },
  });

  const auto kI2cRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "mlb_power"),
  };
  const auto kI2cProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
  };

  const std::vector<fuchsia_driver_framework::ParentSpec2> kParents{{kI2cRules, kI2cProperties}};
  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "ti_ina231_mlb", .parents2 = kParents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request failed to platform bus: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add ti-ina231-mlb composite to platform device: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t AddSpeakerComposite(fdf::WireSyncClient<fpbus::PlatformBus>& pbus,
                                fidl::AnyArena& fidl_arena, fdf::Arena& arena) {
  static const fuchsia_hardware_ti_metadata::Ina231Metadata kMetadata({
      .mode = fuchsia_hardware_ti_metadata::Mode::kShuntAndBusContinuous,
      .shunt_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .bus_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .averages = fuchsia_hardware_ti_metadata::Averages::kAverages1024,
      .shunt_resistance_microohm = 10'000,
      .bus_voltage_limit_microvolt = 11'000'000,
      .alert = fuchsia_hardware_ti_metadata::Alert::kBusUnderVoltage,
      .power_sensor_domain = kPowerSensorDomainAudio,
  });

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  fpbus::Node node({
      .name = "ti-ina231-speakers",
      .vid = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI,
      .pid = bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_NELSON,
      .did = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_INA231_SPEAKERS,
      .metadata =
          std::vector<fpbus::Metadata>{
              {{
                  .id = fuchsia_hardware_ti_metadata::Ina231Metadata::kSerializableName,
                  .data = std::move(persisted_metadata.value()),
              }},
          },
  });

  const auto kI2cRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "audio_power"),
  };
  const auto kI2cProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
  };

  const std::vector<fuchsia_driver_framework::ParentSpec2> kParents{{kI2cRules, kI2cProperties}};
  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "ti_ina231_speakers", .parents2 = kParents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request failed to platform bus: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add ti-ina231-speakers composite to platform device: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Nelson::PowerInit() {
  fidl::Arena<> fidl_arena;
  fdf::Arena mlb_arena('TMLB');
  zx_status_t status = AddMlbComposite(pbus_, fidl_arena, mlb_arena);
  if (status != ZX_OK) {
    return status;
  }

  fdf::Arena speakers_arena('SPKR');
  status = AddSpeakerComposite(pbus_, fidl_arena, speakers_arena);
  if (status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}

zx_status_t Nelson::BrownoutProtectionInit() {
  // Pull up externally.
  gpio_init_steps_.push_back(GpioPull(GPIO_ALERT_PWR_L, fuchsia_hardware_pin::Pull::kNone));

  const ddk::BindRule kGpioRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      ddk::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_10),
  };

  const ddk::BindRule kCodecRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                              bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                              bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_TAS58XX),
  };

  const ddk::BindRule kPowerSensorRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.power.sensor.Service"),
  };

  const ddk::BindRule kGpioInitRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };

  const device_bind_prop_t kGpioProperties[] = {
      ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      ddk::MakeProperty(bind_fuchsia::NAME, "alert-gpio"),
  };

  const device_bind_prop_t kCodecProperties[] = {
      ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
  };

  const device_bind_prop_t kPowerSensorProperties[] = {
      ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.power.sensor.Service"),
      ddk::MakeProperty(bind_fuchsia::ID,
                        bind_fuchsia_amlogic_platform_s905d3::ID_POWER_SENSOR_DOMAIN_AUDIO),
  };

  const device_bind_prop_t kGpioInitProperties[] = {
      ddk::MakeProperty(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };

  zx_status_t status = DdkAddCompositeNodeSpec(
      "brownout_protection", ddk::CompositeNodeSpec(kCodecRules, kCodecProperties)
                                 .AddParentSpec(kGpioRules, kGpioProperties)
                                 .AddParentSpec(kPowerSensorRules, kPowerSensorProperties)
                                 .AddParentSpec(kGpioInitRules, kGpioInitProperties));
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s AddCompositeSpec (brownout-protection)  %d", __FUNCTION__, status);
    return status;
  }

  return ZX_OK;
}

}  // namespace nelson
