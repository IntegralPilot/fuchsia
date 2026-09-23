// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.thermal/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro.h"

namespace {

namespace fpbus = fuchsia_hardware_platform_bus;

fuchsia_hardware_thermal::ThermalTemperatureInfo TripPoint(float temp_c, uint16_t cpu_opp,
                                                           uint16_t gpu_opp) {
  constexpr float kHysteresis = 2.0f;

  return {{
      .up_temp_celsius = temp_c + kHysteresis,
      .down_temp_celsius = temp_c - kHysteresis,
      .fan_level = 0,
      .big_cluster_dvfs_opp = cpu_opp,
      .little_cluster_dvfs_opp = 0,
      .gpu_clk_freq_source = gpu_opp,
  }};
}

zx::result<> CreateThermalPllNode(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  static const std::vector<fpbus::Mmio> kMmios{
      {{
          .base = S905D2_TEMP_SENSOR_PLL_BASE,
          .length = S905D2_TEMP_SENSOR_PLL_LENGTH,
      }},
      {{
          .base = S905D2_TEMP_SENSOR_PLL_TRIM,
          .length = S905D2_TEMP_SENSOR_TRIM_LENGTH,
      }},
      {{
          .base = S905D2_HIU_BASE,
          .length = S905D2_HIU_LENGTH,
      }},
  };

  static const std::vector<fpbus::Irq> kIrqs{
      {{
          .irq = fpbus::IrqSpec::WithIrq(S905D2_TS_PLL_IRQ),
          .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
      }},
  };

  fuchsia_hardware_thermal::ThermalDeviceInfo thermal_config{
      {.active_cooling = false,
       .passive_cooling = false,
       .gpu_throttling = false,
       .num_trip_points = 0,
       .big_little = false,
       .critical_temp_celsius = 101.0,
       .trip_point_info =
           {
               TripPoint(-273.15f, 0, 0),  // 0 Kelvin is impossible, marks end of TripPoints
           },
       .opps = {}}};

  fit::result persisted_metadata = fidl::Persist(thermal_config);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist thermal config: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  std::vector<fpbus::Metadata> metadata{
      {{
          .id = fuchsia_hardware_thermal::ThermalDeviceInfo::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  static const fpbus::Node node{{
      .name = "temperature-sensor-ff634800",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_THERMAL_PLL,
      .mmio = kMmios,
      .irq = kIrqs,
      .metadata = std::move(metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('THER');
  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "temperature-sensor-ff634800", .parents2 = {}}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s",
           result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add node: %s", zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

zx::result<> CreateThermalDdrNode(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  static const std::vector<fpbus::Mmio> kMmios{
      {{
          .base = S905D2_TEMP_SENSOR_DDR_BASE,
          .length = S905D2_TEMP_SENSOR_DDR_LENGTH,
      }},
      {{
          .base = S905D2_TEMP_SENSOR_DDR_TRIM,
          .length = S905D2_TEMP_SENSOR_TRIM_LENGTH,
      }},
      {{
          .base = S905D2_HIU_BASE,
          .length = S905D2_HIU_LENGTH,
      }},
  };

  static const std::vector<fpbus::Irq> kIrqs{
      {{
          .irq = fpbus::IrqSpec::WithIrq(S905D2_TS_DDR_IRQ),
          .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
      }},
  };

  fuchsia_hardware_thermal::ThermalDeviceInfo thermal_config{
      {.active_cooling = false,
       .passive_cooling = false,
       .gpu_throttling = false,
       .num_trip_points = 0,
       .big_little = false,
       .critical_temp_celsius = 110.0,
       .trip_point_info =
           {
               TripPoint(-273.15f, 0, 0),  // 0 Kelvin is impossible, marks end of TripPoints
           },
       .opps = {}}};

  fit::result persisted_metadata = fidl::Persist(thermal_config);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist thermal config: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  std::vector<fpbus::Metadata> metadata{
      {{
          .id = fuchsia_hardware_thermal::ThermalDeviceInfo::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  fpbus::Node node{{
      .name = "temperature-sensor-ff634c00",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_THERMAL_DDR,
      .mmio = kMmios,
      .irq = kIrqs,
      .metadata = std::move(metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('THER');
  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "temperature-sensor-ff634c00", .parents2 = {}}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s",
           result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add node: %s", zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

}  // namespace

namespace astro {

zx_status_t Astro::ThermalInit() {
  if (zx::result result = CreateThermalPllNode(pbus_); result.is_error()) {
    zxlogf(ERROR, "Failed to create thermal pll platform node: %s", result.status_string());
    return result.status_value();
  }

  if (zx::result result = CreateThermalDdrNode(pbus_); result.is_error()) {
    zxlogf(ERROR, "Failed to create thermal ddr platform node: %s", result.status_string());
    return result.status_value();
  }

  return ZX_OK;
}

}  // namespace astro
