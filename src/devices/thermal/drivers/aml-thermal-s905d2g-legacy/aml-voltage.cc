// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-voltage.h"

#include <lib/ddk/debug.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zx/result.h>
#include <string.h>
#include <unistd.h>

#include <memory>

#include <fbl/alloc_checker.h>

namespace thermal {

namespace {

// Sleep for 200 microseconds inorder to let the voltage change
// take effect. Source: Amlogic SDK.
constexpr uint32_t kSleep = 200;
// Step up or down 3 steps in the voltage table while changing
// voltage and not directly. Source: Amlogic SDK
constexpr int kSteps = 3;
// Invalid index in the voltage-table
constexpr int kInvalidIndex = -1;

zx::result<fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>> CreateAndEnablePwm(zx_device_t* parent,
                                                                               const char* name) {
  auto pwm_endpoints = fidl::CreateEndpoints<fuchsia_hardware_pwm::Pwm>();
  if (pwm_endpoints.is_error()) {
    zxlogf(ERROR, "fidl::CreateEndpoints failed - status: %s", pwm_endpoints.status_string());
    return zx::error(pwm_endpoints.status_value());
  }
  zx_status_t status = device_connect_fragment_fidl_protocol(
      parent, name, fuchsia_hardware_pwm::Service::Name, fuchsia_hardware_pwm::Service::Pwm::Name,
      pwm_endpoints->server.TakeChannel().release());
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to get cluster PWM fragment: %s", zx_status_get_string(status));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> client(std::move(pwm_endpoints->client));
  auto result = client->Enable();
  if (!result.ok()) {
    zxlogf(ERROR, "Could not enable PWM: %s", result.status_string());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Could not enable PWM: %s", zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }
  return zx::ok(std::move(client));
}

}  // namespace

zx_status_t AmlVoltageRegulator::Create(
    zx_device_t* parent, const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
    const aml_thermal_info_t* thermal_info) {
  zx::result pdev_client_end = ddk::Device<void>::DdkConnectFragmentFidlProtocol<
      fuchsia_hardware_platform_device::Service::Device>(parent, "pdev");
  if (pdev_client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect to platform device: %s", pdev_client_end.status_string());
    return pdev_client_end.status_value();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  zx::result device_info_result = pdev.GetDeviceInfo();
  if (device_info_result.is_error()) {
    zxlogf(ERROR, "Failed to get device info: %s", device_info_result.status_string());
    return device_info_result.status_value();
  }
  fdf::PDev::DeviceInfo device_info = std::move(device_info_result.value());

  auto pwm_client = CreateAndEnablePwm(parent, "pwm-big-cluster");
  if (pwm_client.is_error()) {
    return pwm_client.status_value();
  }
  big_cluster_pwm_ = std::move(*pwm_client);

  big_little_ = thermal_config.big_little;
  if (big_little_) {
    auto pwm_client = CreateAndEnablePwm(parent, "pwm-little-cluster");
    if (pwm_client.is_error()) {
      return pwm_client.status_value();
    }
    little_cluster_pwm_ = std::move(*pwm_client);
  }

  return Init(thermal_config, thermal_info);
}

zx_status_t AmlVoltageRegulator::Init(
    fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> big_cluster_pwm,
    fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> little_cluster_pwm,
    const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
    const aml_thermal_info_t* thermal_info) {
  big_little_ = thermal_config.big_little;

  big_cluster_pwm_ = std::move(big_cluster_pwm);
  auto result = big_cluster_pwm_->Enable();
  if (!result.ok()) {
    zxlogf(ERROR, "Could not enable PWM: %s", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Could not enable PWM: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  if (big_little_) {
    little_cluster_pwm_ = std::move(little_cluster_pwm);
    auto result = little_cluster_pwm_->Enable();
    if (!result.ok()) {
      zxlogf(ERROR, "Could not enable PWM: %s", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Could not enable PWM: %s", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  return Init(thermal_config, thermal_info);
}

zx_status_t AmlVoltageRegulator::Init(
    const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
    const aml_thermal_info_t* thermal_info) {
  ZX_DEBUG_ASSERT(thermal_info);

  // Get the voltage-table metadata.
  memcpy(&thermal_info_, thermal_info, sizeof(aml_thermal_info_t));

  current_big_cluster_voltage_index_ = kInvalidIndex;
  current_little_cluster_voltage_index_ = kInvalidIndex;

  uint32_t max_big_cluster_microvolt = 0;
  uint32_t max_little_cluster_microvolt = 0;

  const fuchsia_hardware_thermal::wire::OperatingPoint& big_cluster_opp =
      thermal_config.opps[static_cast<uint32_t>(
          fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain)];
  const fuchsia_hardware_thermal::wire::OperatingPoint& little_cluster_opp =
      thermal_config.opps[static_cast<uint32_t>(
          fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain)];

  for (uint32_t i = 0; i < fuchsia_hardware_thermal::wire::kMaxDvfsOpps; i++) {
    if (i < big_cluster_opp.count && big_cluster_opp.opp[i].volt_uv > max_big_cluster_microvolt) {
      max_big_cluster_microvolt = big_cluster_opp.opp[i].volt_uv;
    }
    if (i < little_cluster_opp.count &&
        little_cluster_opp.opp[i].volt_uv > max_little_cluster_microvolt) {
      max_little_cluster_microvolt = little_cluster_opp.opp[i].volt_uv;
    }
  }

  // Set the voltage to maximum to start with
  if (zx_status_t status = SetBigClusterVoltage(max_big_cluster_microvolt); status != ZX_OK) {
    return status;
  }
  if (big_little_) {
    if (zx_status_t status = SetLittleClusterVoltage(max_little_cluster_microvolt);
        status != ZX_OK) {
      return status;
    }
  }

  return ZX_OK;
}

zx_status_t AmlVoltageRegulator::SetClusterVoltage(
    int* current_voltage_index, const fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>& pwm,
    uint32_t microvolt) {
  // Find the entry in the voltage-table.
  int target_index;
  for (target_index = 0; target_index < MAX_VOLTAGE_TABLE; target_index++) {
    if (thermal_info_.voltage_table[target_index].microvolt == microvolt) {
      break;
    }
  }

  // Invalid voltage request.
  if (target_index == MAX_VOLTAGE_TABLE) {
    return ZX_ERR_INVALID_ARGS;
  }

  // If this is the first time we are setting up the voltage
  // we directly set it.
  if (*current_voltage_index < 0) {
    // Update new duty cycle.
    aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
    fuchsia_hardware_pwm::wire::PwmConfig cfg = {
        false, thermal_info_.voltage_pwm_period_ns,
        static_cast<float>(thermal_info_.voltage_table[target_index].duty_cycle),
        fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};

    auto result = pwm->SetConfig(cfg);
    if (!result.ok()) {
      zxlogf(ERROR, "Could not initialize PWM");
      return result.status();
    }
    if (result.value().is_error()) {
      zxlogf(ERROR, "Could not initialize PWM");
      return result.value().error_value();
    }
    usleep(kSleep);
    *current_voltage_index = target_index;
    return ZX_OK;
  }

  // Otherwise we adjust to the target voltage step by step.
  while (*current_voltage_index != target_index) {
    if (*current_voltage_index < target_index) {
      if (*current_voltage_index < target_index - kSteps) {
        // Step up by 3 in the voltage table.
        *current_voltage_index += kSteps;
      } else {
        *current_voltage_index = target_index;
      }
    } else {
      if (*current_voltage_index > target_index + kSteps) {
        // Step down by 3 in the voltage table.
        *current_voltage_index -= kSteps;
      } else {
        *current_voltage_index = target_index;
      }
    }
    // Update new duty cycle.
    aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
    fuchsia_hardware_pwm::wire::PwmConfig cfg = {
        false, thermal_info_.voltage_pwm_period_ns,
        static_cast<float>(thermal_info_.voltage_table[*current_voltage_index].duty_cycle),
        fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};

    auto result = pwm->SetConfig(cfg);
    if (!result.ok()) {
      zxlogf(ERROR, "Could not initialize PWM");
      return result.status();
    }
    if (result.value().is_error()) {
      zxlogf(ERROR, "Could not initialize PWM");
      return result.value().error_value();
    }
    usleep(kSleep);
  }

  return ZX_OK;
}

}  // namespace thermal
