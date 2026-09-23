// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_PWM_DRIVERS_AML_PWM_AML_PWM_H_
#define SRC_DEVICES_PWM_DRIVERS_AML_PWM_AML_PWM_H_

#include <fidl/fuchsia.hardware.pwm/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pwm/cpp/wire.h>
#include <fidl/fuchsia.hardware.pwmimpl/cpp/driver/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <cstring>
#include <vector>

#include <fbl/auto_lock.h>

#include "aml-pwm-regs.h"

namespace pwm {

constexpr uint32_t kPwmPairCount = 2;

class AmlPwm {
 public:
  explicit AmlPwm(fdf::MmioBuffer mmio, fuchsia_hardware_pwm::PwmChannelInfo channel1,
                  fuchsia_hardware_pwm::PwmChannelInfo channel2)
      : channels_{std::move(channel1), std::move(channel2)},
        enabled_{false, false},
        mmio_(std::move(mmio)) {}

  void Init() {
    for (uint32_t i = 0; i < kPwmPairCount; i++) {
      auto& mode_cfg = mode_configs_[i];
      memset(&mode_cfg, 0, sizeof(mode_cfg));
      mode_cfg.mode = Mode::kOff;
      mode_cfg.regular = {};

      const auto& channel = channels_[i];
      const uint8_t* mode_cfg_ptr = reinterpret_cast<const uint8_t*>(&mode_cfg);
      configs_[i] = fuchsia_hardware_pwm::PwmConfig(
          channel.polarity().value_or(false), channel.period_ns().value_or(0), 0.0,
          std::vector<uint8_t>(mode_cfg_ptr, mode_cfg_ptr + sizeof(mode_cfg)));

      const bool should_initialize = !(channel.skip_init().value_or(false));
      if (should_initialize) {
        SetMode(i, Mode::kOff);
      }
    }
  }

  zx::result<fuchsia_hardware_pwm::PwmConfig> GetConfig(uint32_t idx);
  zx_status_t SetConfig(uint32_t idx, const fuchsia_hardware_pwm::PwmConfig& config);
  zx_status_t Enable(uint32_t idx);
  zx_status_t Disable(uint32_t idx);

 private:
  friend class AmlPwmDriver;

  // Register fine control.

  // Sets the PWM controller working mode. `mode` must be a valid PWM mode.
  void SetMode(uint32_t idx, Mode mode);
  // Sets the duty cycle for the default timer.
  // `divider` is the actual PWM clock divider factor in range [1, 128].
  // `duty_cycle` must be a float value in the range [0.0, 100.0].
  void SetDutyCycle(uint32_t idx, int divider, uint32_t period, float duty_cycle);
  // Sets the duty cycle for the second timer in two-timer mode.
  // `divider` is the actual PWM clock divider factor in range [1, 128].
  // `duty_cycle` must be a float value in the range [0.0, 100.0].
  void SetDutyCycle2(uint32_t idx, int divider, uint32_t period, float duty_cycle);
  void Invert(uint32_t idx, bool on);
  void EnableHiZ(uint32_t idx, bool on);
  void EnableClock(uint32_t idx, bool on);
  void EnableConst(uint32_t idx, bool on);
  void SetClock(uint32_t idx, uint8_t sel);
  // Sets the PWM clock divider factor. `divider` must be in range [1, 128].
  void SetClockDivider(uint32_t idx, int divider);
  void EnableBlink(uint32_t idx, bool on);
  void SetBlinkTimes(uint32_t idx, uint8_t times);
  void SetDSSetting(uint32_t idx, uint16_t val);
  void SetTimers(uint32_t idx, uint8_t timer1, uint8_t timer2);

  std::array<fuchsia_hardware_pwm::PwmChannelInfo, kPwmPairCount> channels_;
  std::array<bool, kPwmPairCount> enabled_;
  std::array<fuchsia_hardware_pwm::PwmConfig, kPwmPairCount> configs_;
  std::array<mode_config, kPwmPairCount> mode_configs_;
  std::array<fbl::Mutex, REG_COUNT> locks_;
  fdf::MmioBuffer mmio_;
};

class AmlPwmDriver : public fdf::DriverBase2,
                     public fdf::WireServer<fuchsia_hardware_pwmimpl::PwmImpl> {
 public:
  static constexpr std::string_view kDriverName = "pwm";
  static constexpr std::string_view kChildNodeName = "aml-pwm-device";

  explicit AmlPwmDriver() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  // fdf::WireServer<fuchsia_hardware_pwmimpl::PwmImpl> implementation.
  void GetConfig(GetConfigRequestView request, fdf::Arena& arena,
                 GetConfigCompleter::Sync& completer) override;
  void SetConfig(SetConfigRequestView request, fdf::Arena& arena,
                 SetConfigCompleter::Sync& completer) override;
  void Enable(EnableRequestView request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override;
  void Disable(DisableRequestView request, fdf::Arena& arena,
               DisableCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_pwmimpl::PwmImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  std::vector<std::unique_ptr<AmlPwm>> pwms_;

  size_t max_pwm_id_ = 0;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;

  fdf::ServerBindingGroup<fuchsia_hardware_pwmimpl::PwmImpl> bindings_;
  fdf_metadata::MetadataServer<fuchsia_hardware_pwm::PwmChannelsMetadata> metadata_server_;
};

}  // namespace pwm

#endif  // SRC_DEVICES_PWM_DRIVERS_AML_PWM_AML_PWM_H_
