// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_THERMAL_DRIVERS_AML_THERMAL_S905D2G_LEGACY_AML_VOLTAGE_H_
#define SRC_DEVICES_THERMAL_DRIVERS_AML_THERMAL_S905D2G_LEGACY_AML_VOLTAGE_H_

#include <fidl/fuchsia.hardware.pwm/cpp/wire.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>

#include <ddktl/device.h>
#include <fbl/macros.h>
#include <soc/aml-common/aml-pwm-regs.h>
#include <soc/aml-common/aml-thermal.h>

namespace thermal {
// This class represents a voltage regulator
// on the Amlogic board which provides interface
// to set and get current voltage for the CPU.
class AmlVoltageRegulator {
 public:
  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(AmlVoltageRegulator);
  AmlVoltageRegulator() = default;
  zx_status_t Create(zx_device_t* parent,
                     const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
                     const aml_thermal_info_t* thermal_info);
  // For testing
  zx_status_t Init(fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> big_cluster_pwm,
                   fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> little_cluster_pwm,
                   const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
                   const aml_thermal_info_t* thermal_info);
  zx_status_t Init(const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
                   const aml_thermal_info_t* thermal_info);
  uint32_t GetVoltage(fuchsia_hardware_thermal::wire::PowerDomain power_domain) {
    if (power_domain == fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain) {
      if (current_big_cluster_voltage_index_ < 0 ||
          current_big_cluster_voltage_index_ >= MAX_VOLTAGE_TABLE) {
        return 0;
      }
      return thermal_info_.voltage_table[current_big_cluster_voltage_index_].microvolt;
    }

    if (power_domain == fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain &&
        big_little_) {
      if (current_little_cluster_voltage_index_ < 0 ||
          current_little_cluster_voltage_index_ >= MAX_VOLTAGE_TABLE) {
        return 0;
      }
      return thermal_info_.voltage_table[current_little_cluster_voltage_index_].microvolt;
    }

    return 0;
  }

  zx_status_t SetVoltage(fuchsia_hardware_thermal::wire::PowerDomain power_domain,
                         uint32_t microvolt) {
    if (power_domain == fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain) {
      return SetClusterVoltage(&current_big_cluster_voltage_index_, big_cluster_pwm_, microvolt);
    }

    if (power_domain == fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain) {
      if (!big_little_) {
        return ZX_ERR_NOT_SUPPORTED;
      }
      return SetClusterVoltage(&current_little_cluster_voltage_index_, little_cluster_pwm_,
                               microvolt);
    }

    return ZX_ERR_INVALID_ARGS;
  }

 private:
  enum {
    FRAGMENT_PDEV = 0,
    FRAGMENT_PWM_BIG_CLUSTER = 1,
    FRAGMENT_PWM_LITTLE_CLUSTER = 2,
    FRAGMENT_COUNT = 3,
  };

  zx_status_t SetClusterVoltage(int* current_voltage_index,
                                const fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>& pwm,
                                uint32_t microvolt);
  zx_status_t SetBigClusterVoltage(uint32_t microvolt) {
    return SetClusterVoltage(&current_big_cluster_voltage_index_, big_cluster_pwm_, microvolt);
  }
  zx_status_t SetLittleClusterVoltage(uint32_t microvolt) {
    return SetClusterVoltage(&current_little_cluster_voltage_index_, little_cluster_pwm_,
                             microvolt);
  }

  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> big_cluster_pwm_;
  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> little_cluster_pwm_;
  aml_thermal_info_t thermal_info_;
  int current_big_cluster_voltage_index_ = -1;
  int current_little_cluster_voltage_index_ = -1;
  bool big_little_ = false;
};
}  // namespace thermal

#endif  // SRC_DEVICES_THERMAL_DRIVERS_AML_THERMAL_S905D2G_LEGACY_AML_VOLTAGE_H_
