// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.thermal/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/pwm/cpp/bind.h>
#include <ddk/metadata/camera.h>
#include <soc/aml-common/aml-thermal.h>
#include <soc/aml-meson/g12b-clk.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>
#include <soc/aml-t931/t931-pwm.h>

#include "sherlock.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {

constexpr uint32_t kPwmDFn = 3;

const std::vector<fpbus::Mmio> thermal_mmios_pll{
    {{
        .base = T931_TEMP_SENSOR_PLL_BASE,
        .length = T931_TEMP_SENSOR_PLL_LENGTH,
    }},
    {{
        .base = T931_TEMP_SENSOR_PLL_TRIM,
        .length = T931_TEMP_SENSOR_TRIM_LENGTH,
    }},
    {{
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
    }},
};

const std::vector<fpbus::Mmio> thermal_mmios_ddr{
    {{
        .base = T931_TEMP_SENSOR_DDR_BASE,
        .length = T931_TEMP_SENSOR_DDR_LENGTH,
    }},
    {{
        .base = T931_TEMP_SENSOR_DDR_TRIM,
        .length = T931_TEMP_SENSOR_TRIM_LENGTH,
    }},
    {{
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
    }},
};

const std::vector<fpbus::Irq> thermal_irqs_pll{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_TS_PLL_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

const std::vector<fpbus::Irq> thermal_irqs_ddr{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_TS_DDR_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

fuchsia_hardware_thermal::ThermalTemperatureInfo TripPoint(float temp_c, uint16_t cpu_opp_big,
                                                           uint16_t cpu_opp_little,
                                                           uint16_t gpu_opp) {
  constexpr float kHysteresis = 2.0f;

  return {{
      .up_temp_celsius = temp_c + kHysteresis,
      .down_temp_celsius = temp_c - kHysteresis,
      .fan_level = 0,
      .big_cluster_dvfs_opp = cpu_opp_big,
      .little_cluster_dvfs_opp = cpu_opp_little,
      .gpu_clk_freq_source = gpu_opp,
  }};
}

// clang-format on
aml_thermal_info_t aml_thermal_info = {
    .voltage_table =
        {
            {1'022'000, 0}, {1'011'000, 3}, {1'001'000, 6}, {991'000, 10}, {981'000, 13},
            {971'000, 16},  {961'000, 20},  {951'000, 23},  {941'000, 26}, {931'000, 30},
            {921'000, 33},  {911'000, 36},  {901'000, 40},  {891'000, 43}, {881'000, 46},
            {871'000, 50},  {861'000, 53},  {851'000, 56},  {841'000, 60}, {831'000, 63},
            {821'000, 67},  {811'000, 70},  {801'000, 73},  {791'000, 76}, {781'000, 80},
            {771'000, 83},  {761'000, 86},  {751'000, 90},  {741'000, 93}, {731'000, 96},
            {721'000, 100},
        },
    .initial_cluster_frequencies =
        {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
            [static_cast<uint32_t>(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain)] =
                1'000'000'000,
            [static_cast<uint32_t>(
                fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain)] =
                1'200'000'000,
#pragma GCC diagnostic pop
        },
    .voltage_pwm_period_ns = 1250,
    .opps = {},
    .cluster_id_map = {},
};

const std::map<uint32_t, std::string> kPwmIdMap = {
    {T931_PWM_A, "pwm-big-cluster"},
    {T931_PWM_AO_D, "pwm-little-cluster"},
};

const std::map<uint32_t, std::string> kClockFunctionMap = {
    {g12b_clk::G12B_CLK_SYS_PLL_DIV16, "sys-pll-div16"},
    {g12b_clk::G12B_CLK_SYS_PLLB_DIV16, "sys-pllb-div16"},
    {g12b_clk::G12B_CLK_SYS_CPU_CLK_DIV16, "sys-cpu-div16"},
    {g12b_clk::G12B_CLK_SYS_CPUB_CLK_DIV16, "sys-cpub-div16"},
};

zx::result<> CreateThermalPllNode(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  /*
   * PASSIVE COOLING - For Sherlock, we have DVFS support added
   *
   * Below is the operating point information for Small cluster
   * Operating point 0  - Freq 0.1000 GHz Voltage 0.7310 V
   * Operating point 1  - Freq 0.2500 GHz Voltage 0.7310 V
   * Operating point 2  - Freq 0.5000 GHz Voltage 0.7310 V
   * Operating point 3  - Freq 0.6670 GHz Voltage 0.7310 V
   * Operating point 4  - Freq 1.0000 GHz Voltage 0.7310 V
   * Operating point 5  - Freq 1.2000 GHz Voltage 0.7310 V
   * Operating point 6  - Freq 1.3980 GHz Voltage 0.7610 V
   * Operating point 7  - Freq 1.5120 GHz Voltage 0.7910 V
   * Operating point 8  - Freq 1.6080 GHz Voltage 0.8310 V
   * Operating point 9  - Freq 1.7040 GHz Voltage 0.8610 V
   * Operating point 10 - Freq 1.8960 GHz Voltage 0.9810 V
   *
   * Below is the operating point information for Big cluster
   * Operating point 0  - Freq 0.1000 GHz Voltage 0.7510 V
   * Operating point 1  - Freq 0.2500 GHz Voltage 0.7510 V
   * Operating point 2  - Freq 0.5000 GHz Voltage 0.7510 V
   * Operating point 3  - Freq 0.6670 GHz Voltage 0.7510 V
   * Operating point 4  - Freq 1.0000 GHz Voltage 0.7710 V
   * Operating point 5  - Freq 1.2000 GHz Voltage 0.7710 V
   * Operating point 6  - Freq 1.3980 GHz Voltage 0.7910 V
   * Operating point 7  - Freq 1.5120 GHz Voltage 0.8210 V
   * Operating point 8  - Freq 1.6080 GHz Voltage 0.8610 V
   * Operating point 9  - Freq 1.7040 GHz Voltage 0.8910 V
   * Operating point 10 - Freq 1.9080 GHz Voltage 1.0220 V
   *
   * GPU_CLK_FREQUENCY_SOURCE -
   * 0 - 285.7 MHz
   * 1 - 400 MHz
   * 2 - 500 MHz
   * 3 - 666 MHz
   * 4 - 800 MHz
   */

  // NOTE: This is a very trivial policy, no data backing it up
  // As we do more testing this policy can evolve.
  fuchsia_hardware_thermal::ThermalDeviceInfo thermal_config{
      {
          .active_cooling = false,
          .passive_cooling = true,
          .gpu_throttling = true,
          .num_trip_points = 4,
          .big_little = true,
          .critical_temp_celsius = 101.0f,
          .trip_point_info =
              {
                  TripPoint(82.5f, 9, 10, 4),
                  TripPoint(85.0f, 8, 9, 4),
                  TripPoint(87.5f, 6, 6, 4),
                  TripPoint(90.0f, 4, 4, 4),
                  TripPoint(-273.15f, 0, 0, 0),  // 0 Kelvin is impossible, marks end of TripPoints
              },
          .opps =
              {
                  fuchsia_hardware_thermal::OperatingPoint{{
                      .opp =
                          {
                              fuchsia_hardware_thermal::OperatingPointEntry{
                                  {.freq_hz = 100'000'000, .volt_uv = 751'000}},
                              {{.freq_hz = 250'000'000, .volt_uv = 751'000}},
                              {{.freq_hz = 500'000'000, .volt_uv = 751'000}},
                              {{.freq_hz = 667'000'000, .volt_uv = 751'000}},
                              {{.freq_hz = 1'000'000'000, .volt_uv = 771'000}},
                              {{.freq_hz = 1'200'000'000, .volt_uv = 771'000}},
                              {{.freq_hz = 1'398'000'000, .volt_uv = 791'000}},
                              {{.freq_hz = 1'512'000'000, .volt_uv = 821'000}},
                              {{.freq_hz = 1'608'000'000, .volt_uv = 861'000}},
                              {{.freq_hz = 1'704'000'000, .volt_uv = 891'000}},
                              {{.freq_hz = 1'908'000'000, .volt_uv = 1'022'000}},
                          },
                      .latency = 0,
                      .count = 11,
                  }},
                  fuchsia_hardware_thermal::OperatingPoint{
                      {
                          .opp =
                              {
                                  fuchsia_hardware_thermal::OperatingPointEntry{
                                      {.freq_hz = 100'000'000, .volt_uv = 731'000}},
                                  {{.freq_hz = 250'000'000, .volt_uv = 731'000}},
                                  {{.freq_hz = 500'000'000, .volt_uv = 731'000}},
                                  {{.freq_hz = 667'000'000, .volt_uv = 731'000}},
                                  {{.freq_hz = 1'000'000'000, .volt_uv = 731'000}},
                                  {{.freq_hz = 1'200'000'000, .volt_uv = 731'000}},
                                  {{.freq_hz = 1'398'000'000, .volt_uv = 761'000}},
                                  {{.freq_hz = 1'512'000'000, .volt_uv = 791'000}},
                                  {{.freq_hz = 1'608'000'000, .volt_uv = 831'000}},
                                  {{.freq_hz = 1'704'000'000, .volt_uv = 861'000}},
                                  {{.freq_hz = 1'896'000'000, .volt_uv = 1'011'000}},
                              },
                          .latency = 0,
                          .count = 11,
                      }},
              },
      }};

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
      {{
          .id = std::to_string(DEVICE_METADATA_PRIVATE),
          .data = std::vector<uint8_t>(
              reinterpret_cast<const uint8_t*>(&aml_thermal_info),
              reinterpret_cast<const uint8_t*>(&aml_thermal_info) + sizeof(aml_thermal_info)),
      }},
  };

  const fpbus::Node node{{
      .name = "temperature-sensor-ff634800",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_T931,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_THERMAL_PLL,
      .mmio = thermal_mmios_pll,
      .irq = thermal_irqs_pll,
      .metadata = std::move(metadata),
  }};

  // The PLL sensor is controlled by a legacy thermal device, which performs DVFS.
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('SHER');

  std::vector<fuchsia_driver_framework::ParentSpec2> parents;
  parents.reserve(kClockFunctionMap.size() + kPwmIdMap.size() + 1);
  parents.push_back(fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP,
                                      bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
          },
  }});

  for (auto& [pwm_id, function] : kPwmIdMap) {
    auto rules = std::vector{
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, pwm_id),
    };
    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, function),
    };
    parents.push_back(fuchsia_driver_framework::ParentSpec2{{rules, properties}});
  }

  for (auto& [clock_id, function] : kClockFunctionMap) {
    auto rules = std::vector{
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, clock_id),
    };
    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, function),
    };
    parents.push_back(fuchsia_driver_framework::ParentSpec2{{rules, properties}});
  }

  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "temperature-sensor-ff634800", .parents2 = parents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send NodeAdd request: %s", result.FormatDescription().data());
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
  fuchsia_hardware_thermal::ThermalDeviceInfo thermal_config{
      {.active_cooling = false,
       .passive_cooling = false,
       .gpu_throttling = false,
       .num_trip_points = 0,
       .big_little = false,
       .critical_temp_celsius = 110.0,
       .trip_point_info = {TripPoint(-273.15f, 0, 0, 0)},  // Unused
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
      .vid = PDEV_VID_AMLOGIC,
      .pid = PDEV_PID_AMLOGIC_T931,
      .did = PDEV_DID_AMLOGIC_THERMAL_DDR,
      .mmio = thermal_mmios_ddr,
      .irq = thermal_irqs_ddr,
      .metadata = std::move(metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('SHER');

  // The DDR sensor is controlled by a non-legacy thermal device, which only reads temperature.
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

zx_status_t Sherlock::ThermalInit() {
  // Configure the GPIO to be Output & set it to alternate
  // function 3 which puts in PWM_D mode. A53 cluster (Small)
  gpio_init_steps_.push_back(GpioFunction(T931_GPIOE(1), kPwmDFn));

  gpio_init_steps_.push_back(GpioOutput(T931_GPIOE(1), false));

  // Configure the GPIO to be Output & set it to alternate
  // function 3 which puts in PWM_D mode. A73 cluster (Big)
  gpio_init_steps_.push_back(GpioFunction(T931_GPIOE(2), kPwmDFn));

  gpio_init_steps_.push_back(GpioOutput(T931_GPIOE(2), false));

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

}  // namespace sherlock
