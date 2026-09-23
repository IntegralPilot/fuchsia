// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-pwm.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstdint>
#include <cstring>
#include <limits>
#include <span>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <soc/aml-a1/a1-pwm.h>
#include <soc/aml-a113/a113-pwm.h>
#include <soc/aml-a5/a5-pwm.h>
#include <soc/aml-common/aml-pwm-regs.h>
#include <soc/aml-s905d2/s905d2-pwm.h>
#include <soc/aml-t931/t931-pwm.h>

#include "pwm_parser.h"

namespace pwm {

namespace {

// Input clock frequency
constexpr uint32_t kReferenceClockFrequencyHz = 24'000'000;
// Nanoseconds per second
constexpr uint32_t kNsecPerSec = 1'000'000'000;

constexpr int64_t DivideRounded(int64_t num, int64_t denom) { return (num + (denom / 2)) / denom; }

constexpr int kMinimumDivider = 1;
constexpr int kMaximumDivider = 0x7f + 1;

// Below explains how `kMaximumAllowedPeriodNs` is calculated and we statically
// verify the value.
//
// High / low time of a PWM period is stored in registers as numbers of clock
// periods and dividers:
//
//   High time (ns) = <NUMBER OF CLOCK PERIODS> (16 bits unsigned)
//                  * (Nanoseconds per clock period)
//                  * (<DIVIDER> + 1) (<DIVIDER> is 7-bit unsigned)
//
// So the maximum allowed PWM period is the maximum value of High (low) time.
//
//   0xffff * (Nanoseconds per clock period) * (0x7f + 1)  (unit: ns)
static_assert([] {
  constexpr int64_t kNanosecondsPerClock = kNsecPerSec / kReferenceClockFrequencyHz;
  constexpr int kMaxClockPeriodCount = std::numeric_limits<uint16_t>::max();

  return kMaximumAllowedPeriodNs == kNanosecondsPerClock * kMaximumDivider * kMaxClockPeriodCount;
}());

// Gets the minimum allowed divider value from given `period_ns`.
//
// `period_ns` must be not greater than kMaximumAllowedPeriodNs.
int GetDividerFromPeriodNs(int64_t period_ns) {
  ZX_ASSERT(period_ns <= kMaximumAllowedPeriodNs);
  constexpr int64_t kNanosecondsPerClock = kNsecPerSec / kReferenceClockFrequencyHz;
  for (int divider = kMinimumDivider; divider <= kMaximumDivider; divider++) {
    const int64_t period_count = DivideRounded(period_ns, kNanosecondsPerClock * divider);
    if (period_count <= std::numeric_limits<uint16_t>::max()) {
      return divider;
    }
  }
  // Given that period_ns never exceeds the maximum allowed value, this should
  // never happen.
  ZX_ASSERT(false);
}

// Gets the minimum allowed divider value from given `config`, fulfilling
// requirements for all the timers.
//
// `period_ns` and `period_ns2` (for two-timer mode) must be not greater than
// kMaximumAllowedPeriodNs.
int GetDividerFromConfig(const fuchsia_hardware_pwm::PwmConfig& config) {
  ZX_ASSERT(config.mode_config().size() == sizeof(mode_config));
  const auto* mode_cfg = reinterpret_cast<const mode_config*>(config.mode_config().data());
  switch (mode_cfg->mode) {
    case Mode::kOff:
      return 1;
    case Mode::kOn:
    case Mode::kDeltaSigma: {
      ZX_ASSERT(config.period_ns() <= kMaximumAllowedPeriodNs);
      return GetDividerFromPeriodNs(config.period_ns());
    }
    case Mode::kTwoTimer: {
      ZX_ASSERT(config.period_ns() <= kMaximumAllowedPeriodNs);
      ZX_ASSERT(mode_cfg->two_timer.period_ns2 <= kMaximumAllowedPeriodNs);
      const int divider_first_timer = GetDividerFromPeriodNs(config.period_ns());
      const int divider_second_timer = GetDividerFromPeriodNs(mode_cfg->two_timer.period_ns2);
      return std::max(divider_first_timer, divider_second_timer);
    }
  }
}

struct DutyCycleClockCount {
  int64_t high_count;
  int64_t low_count;
};

DutyCycleClockCount DutyCycleToClockCount(int divider, float duty_cycle, int64_t period_ns) {
  constexpr int64_t kNanosecondsPerClock = kNsecPerSec / kReferenceClockFrequencyHz;

  // Calculate the high and low count first based on the duty cycle requested.
  const int64_t high_time_ns =
      DivideRounded(static_cast<int64_t>(duty_cycle * static_cast<float>(period_ns)), 100);
  const int64_t period_count = DivideRounded(period_ns, kNanosecondsPerClock * divider);
  const int64_t duty_count = DivideRounded(high_time_ns, kNanosecondsPerClock * divider);

  int64_t high_count = duty_count;
  int64_t low_count = period_count - duty_count;
  if (duty_count != period_count && duty_count != 0) {
    high_count--;
    low_count--;
  }

  return DutyCycleClockCount{
      .high_count = high_count,
      .low_count = low_count,
  };
}

bool IsValidConfig(const fuchsia_hardware_pwm::PwmConfig& config) {
  if (config.mode_config().size() != sizeof(mode_config)) {
    fdf::error("Mode config has incorrect size: expected {} bytes, actual is {} bytes",
               sizeof(mode_config), config.mode_config().size());
    return false;
  }

  const auto* mode_cfg = reinterpret_cast<const mode_config*>(config.mode_config().data());
  const Mode mode = mode_cfg->mode;
  switch (mode) {
    case Mode::kOff:
      return true;
    case Mode::kTwoTimer:
      if (mode_cfg->two_timer.duty_cycle2 < 0.0f || mode_cfg->two_timer.duty_cycle2 > 100.0f) {
        fdf::error("timer #2 duty cycle ({:.3}) is not in [0.0, 100.0]",
                   mode_cfg->two_timer.duty_cycle2);
        return false;
      }
      if (mode_cfg->two_timer.period_ns2 > kMaximumAllowedPeriodNs) {
        fdf::error("timer #2 period ({} ns) exceeds the maximum allowed period ({} ns)",
                   mode_cfg->two_timer.period_ns2, kMaximumAllowedPeriodNs);
        return false;
      }
      [[fallthrough]];
    case Mode::kOn:
    case Mode::kDeltaSigma:
      if (config.duty_cycle() < 0.0f || config.duty_cycle() > 100.0f) {
        fdf::error("timer #1 duty cycle ({:.3}) is not in [0.0, 100.0]", config.duty_cycle());
        return false;
      }
      if (config.period_ns() > kMaximumAllowedPeriodNs) {
        fdf::error("timer #1 period ({} ns) exceeds the maximum allowed period ({} ns)",
                   config.period_ns(), kMaximumAllowedPeriodNs);
        return false;
      }
      break;
    default:
      fdf::error("Unsupported mode ({})", static_cast<uint32_t>(mode));
      return false;
  }
  return true;
}

fuchsia_hardware_pwm::PwmChannelsMetadata ConvertMetadata(
    const pwm_metadata::PwmMetadata& generic) {
  std::vector<fuchsia_hardware_pwm::PwmChannelInfo> channels;
  for (const auto& c : generic.channels) {
    fuchsia_hardware_pwm::PwmChannelInfo info;
    info.id(c.channel);
    info.period_ns(c.period_ns);
    channels.push_back(std::move(info));
  }
  return {{.channels = std::move(channels)}};
}

}  // namespace

zx::result<fuchsia_hardware_pwm::PwmConfig> AmlPwm::GetConfig(uint32_t idx) {
  if (idx > 1) {
    fdf::error("Invalid index: {}", idx);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(configs_[idx]);
}

zx_status_t AmlPwm::SetConfig(uint32_t idx, const fuchsia_hardware_pwm::PwmConfig& config) {
  if (idx > 1) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!IsValidConfig(config)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Save old config
  fuchsia_hardware_pwm::PwmConfig old_config = configs_[idx];

  // Update new
  configs_[idx] = config;

  const auto* mode_cfg = reinterpret_cast<const mode_config*>(config.mode_config().data());
  const Mode mode = mode_cfg->mode;

  const auto* old_mode_cfg = reinterpret_cast<const mode_config*>(old_config.mode_config().data());

  bool mode_eq = (old_mode_cfg->mode == mode);
  if (!mode_eq) {
    SetMode(idx, mode);
  }

  if (mode == Mode::kOff) {
    return ZX_OK;
  }

  int old_divider = GetDividerFromConfig(old_config);
  int new_divider = GetDividerFromConfig(config);
  bool divider_eq = old_divider == new_divider;
  if (!(mode_eq && divider_eq)) {
    SetClockDivider(idx, new_divider);
  }

  bool en_const = (config.duty_cycle() == 0 || config.duty_cycle() == 100);
  bool val_eq;

  if (mode == Mode::kDeltaSigma) {
    val_eq = (old_mode_cfg->delta_sigma.delta == mode_cfg->delta_sigma.delta);
    if (!(mode_eq && val_eq)) {
      SetDSSetting(idx, mode_cfg->delta_sigma.delta);
    }
  }

  if (mode == Mode::kTwoTimer) {
    en_const = (en_const || mode_cfg->two_timer.duty_cycle2 == 0 ||
                mode_cfg->two_timer.duty_cycle2 == 100);

    val_eq = (old_mode_cfg->two_timer.period_ns2 == mode_cfg->two_timer.period_ns2) &&
             (old_mode_cfg->two_timer.duty_cycle2 == mode_cfg->two_timer.duty_cycle2);
    if (!(mode_eq && divider_eq && val_eq)) {
      SetDutyCycle2(idx, new_divider, mode_cfg->two_timer.period_ns2,
                    mode_cfg->two_timer.duty_cycle2);
    }

    val_eq = (old_mode_cfg->two_timer.timer1 == mode_cfg->two_timer.timer1) &&
             (old_mode_cfg->two_timer.timer2 == mode_cfg->two_timer.timer2);
    if (!(mode_eq && val_eq)) {
      SetTimers(idx, mode_cfg->two_timer.timer1, mode_cfg->two_timer.timer2);
    }
  }

  val_eq = (old_config.polarity() == config.polarity());
  if (!(mode_eq && val_eq)) {
    Invert(idx, config.polarity());
  }
  EnableConst(idx, en_const);

  val_eq = (old_config.period_ns() == config.period_ns()) &&
           (old_config.duty_cycle() == config.duty_cycle());
  if (!(mode_eq && divider_eq && val_eq)) {
    SetDutyCycle(idx, new_divider, config.period_ns(), config.duty_cycle());
  }

  return ZX_OK;
}

zx_status_t AmlPwm::Enable(uint32_t idx) {
  if (idx > 1) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!enabled_[idx]) {
    EnableClock(idx, true);
    enabled_[idx] = true;
  }
  return ZX_OK;
}

zx_status_t AmlPwm::Disable(uint32_t idx) {
  if (idx > 1) {
    return ZX_ERR_INVALID_ARGS;
  }
  zx_status_t status = ZX_OK;
  if (enabled_[idx]) {
    EnableClock(idx, false);
    enabled_[idx] = false;
  }
  return status;
}

void AmlPwm::SetMode(uint32_t idx, Mode mode) {
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_en_b(mode == Mode::kOn || mode == Mode::kTwoTimer)
        .set_ds_en_b(mode == Mode::kDeltaSigma)
        .set_en_b2(mode == Mode::kTwoTimer);
  } else {
    misc_reg.set_en_a(mode == Mode::kOn || mode == Mode::kTwoTimer)
        .set_ds_en_a(mode == Mode::kDeltaSigma)
        .set_en_a2(mode == Mode::kTwoTimer);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::SetDutyCycle(uint32_t idx, int divider, uint32_t period_ns, float duty_cycle) {
  ZX_ASSERT(duty_cycle >= 0.0f);
  ZX_ASSERT(duty_cycle <= 100.0f);
  ZX_ASSERT(divider >= 1);
  ZX_ASSERT(divider <= 128);

  // Write duty cycle to registers
  DutyCycleClockCount clock_count = DutyCycleToClockCount(divider, duty_cycle, period_ns);
  if (clock_count.high_count < std::numeric_limits<uint16_t>::min() ||
      clock_count.high_count > std::numeric_limits<uint16_t>::max()) {
    fdf::error("Invalid high count: {}", clock_count.high_count);
  }
  if (clock_count.low_count < std::numeric_limits<uint16_t>::min() ||
      clock_count.low_count > std::numeric_limits<uint16_t>::max()) {
    fdf::error("Invalid low count: {}", clock_count.low_count);
  }

  if (idx % 2) {
    fbl::AutoLock lock(&locks_[REG_B]);
    DutyCycleReg::GetB()
        .ReadFrom(&mmio_)
        .set_high(static_cast<unsigned int>(clock_count.high_count))
        .set_low(static_cast<unsigned int>(clock_count.low_count))
        .WriteTo(&mmio_);
  } else {
    fbl::AutoLock lock(&locks_[REG_A]);
    DutyCycleReg::GetA()
        .ReadFrom(&mmio_)
        .set_high(static_cast<unsigned int>(clock_count.high_count))
        .set_low(static_cast<unsigned int>(clock_count.low_count))
        .WriteTo(&mmio_);
  }
}

void AmlPwm::SetDutyCycle2(uint32_t idx, int divider, uint32_t period_ns, float duty_cycle) {
  ZX_ASSERT(duty_cycle >= 0.0f);
  ZX_ASSERT(duty_cycle <= 100.0f);
  ZX_ASSERT(divider >= 1);
  ZX_ASSERT(divider <= 128);

  // Write duty cycle to registers
  DutyCycleClockCount clock_count = DutyCycleToClockCount(divider, duty_cycle, period_ns);
  if (clock_count.high_count < std::numeric_limits<uint16_t>::min() ||
      clock_count.high_count > std::numeric_limits<uint16_t>::max()) {
    fdf::error("Invalid high count: {}", clock_count.high_count);
  }
  if (clock_count.low_count < std::numeric_limits<uint16_t>::min() ||
      clock_count.low_count > std::numeric_limits<uint16_t>::max()) {
    fdf::error("Invalid low count: {}", clock_count.low_count);
  }

  if (idx % 2) {
    fbl::AutoLock lock(&locks_[REG_B2]);
    DutyCycleReg::GetB2()
        .ReadFrom(&mmio_)
        .set_high(static_cast<unsigned int>(clock_count.high_count))
        .set_low(static_cast<unsigned int>(clock_count.low_count))
        .WriteTo(&mmio_);
  } else {
    fbl::AutoLock lock(&locks_[REG_A2]);
    DutyCycleReg::GetA2()
        .ReadFrom(&mmio_)
        .set_high(static_cast<unsigned int>(clock_count.high_count))
        .set_low(static_cast<unsigned int>(clock_count.low_count))
        .WriteTo(&mmio_);
  }
}

void AmlPwm::Invert(uint32_t idx, bool on) {
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_inv_en_b(on);
  } else {
    misc_reg.set_inv_en_a(on);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::EnableHiZ(uint32_t idx, bool on) {
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_hiz_b(on);
  } else {
    misc_reg.set_hiz_a(on);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::EnableClock(uint32_t idx, bool on) {
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_clk_en_b(on);
  } else {
    misc_reg.set_clk_en_a(on);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::EnableConst(uint32_t idx, bool on) {
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_constant_en_b(on);
  } else {
    misc_reg.set_constant_en_a(on);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::SetClock(uint32_t idx, uint8_t sel) {
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_clk_sel_b(sel);
  } else {
    misc_reg.set_clk_sel_a(sel);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::SetClockDivider(uint32_t idx, int divider) {
  ZX_ASSERT(divider >= 1);
  ZX_ASSERT(divider <= 128);
  int divider_select = divider - 1;
  fbl::AutoLock lock(&locks_[REG_MISC]);
  auto misc_reg = MiscReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    misc_reg.set_clk_div_b(divider_select);
  } else {
    misc_reg.set_clk_div_a(divider_select);
  }
  misc_reg.WriteTo(&mmio_);
}

void AmlPwm::EnableBlink(uint32_t idx, bool on) {
  fbl::AutoLock lock(&locks_[REG_BLINK]);
  auto blink_reg = BlinkReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    blink_reg.set_enable_b(on);
  } else {
    blink_reg.set_enable_a(on);
  }
  blink_reg.WriteTo(&mmio_);
}

void AmlPwm::SetBlinkTimes(uint32_t idx, uint8_t times) {
  fbl::AutoLock lock(&locks_[REG_BLINK]);
  auto blink_reg = BlinkReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    blink_reg.set_times_b(times);
  } else {
    blink_reg.set_times_a(times);
  }
  blink_reg.WriteTo(&mmio_);
}

void AmlPwm::SetDSSetting(uint32_t idx, uint16_t val) {
  fbl::AutoLock lock(&locks_[REG_DS]);
  auto ds_reg = DeltaSigmaReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    ds_reg.set_b(val);
  } else {
    ds_reg.set_a(val);
  }
  ds_reg.WriteTo(&mmio_);
}

void AmlPwm::SetTimers(uint32_t idx, uint8_t timer1, uint8_t timer2) {
  fbl::AutoLock lock(&locks_[REG_TIME]);
  auto time_reg = TimeReg::Get().ReadFrom(&mmio_);
  if (idx % 2) {
    time_reg.set_b1(timer1).set_b2(timer2);
  } else {
    time_reg.set_a1(timer1).set_a2(timer2);
  }
  time_reg.WriteTo(&mmio_);
}

zx::result<> AmlPwmDriver::Start(fdf::DriverContext context) {
  zx::result pdev_client_end =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end.status_string());
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client_end.value()));

  zx::result device_info = pdev.GetDeviceInfo();
  if (device_info.is_error()) {
    fdf::error("Failed to get device info : {}", device_info.status_string());
    return device_info.take_error();
  }

  std::vector<fdf::MmioBuffer> mmios;
  for (uint32_t i = 0; i < device_info->mmio_count; i++) {
    zx::result mmio = pdev.MapMmio(i);
    if (mmio.is_error()) {
      fdf::error("Failed to get mmio for index {}: {}", i, mmio.status_string());
      return mmio.take_error();
    }
    mmios.push_back(std::move(*mmio));
  }

  fuchsia_hardware_pwm::PwmChannelsMetadata metadata;
  {
    std::optional<fuchsia_hardware_pwm::PwmChannelsMetadata> parsed_metadata;
    // Try to get generic metadata first
    zx::result generic_res = pdev.GetFidlMetadata<fuchsia_driver_metadata::Dictionary>(
        "fuchsia.hardware.pwm.PwmChannelsMetadata");
    if (generic_res.is_ok()) {
      const std::optional parsed = pwm_metadata::PwmMetadata::Parse(generic_res.value());
      if (parsed) {
        parsed_metadata = ConvertMetadata(*parsed);
      } else {
        fdf::error("Failed to parse generic PWM metadata");
      }
    }

    if (!parsed_metadata.has_value()) {
      // Fall back to old metadata
      zx::result metadata_res = pdev.GetFidlMetadata<fuchsia_hardware_pwm::PwmChannelsMetadata>();
      if (metadata_res.is_error()) {
        fdf::error("Failed to get metadata: {}", metadata_res.status_string());
        return metadata_res.take_error();
      }
      metadata = std::move(*metadata_res);
    } else {
      metadata = std::move(*parsed_metadata);
    }
  }

  if (zx::result result = metadata_server_.Serve(*outgoing(), dispatcher(), metadata);
      result.is_error()) {
    fdf::error("Failed to serve metadata: {}", result);
    return result.take_error();
  }

  // PWM IDs are expected to be continuous starting with 0. There will be 2 PWM ID per mmio.
  max_pwm_id_ = (mmios.size() * 2) - 1;
  std::vector<fuchsia_hardware_pwm::PwmChannelInfo> supported_pwm_channels;
  supported_pwm_channels.resize(mmios.size() * 2lu);
  for (uint32_t i = 0; i < supported_pwm_channels.size(); i++) {
    supported_pwm_channels[i].id() = i;
  }

  // Validate the pwm ids passed in and copy config information. The config is left as default
  // for the rest of pwms which are not part of the metadata.
  if (!metadata.channels().has_value()) {
    fdf::error("Metadata missing channels field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  const std::span<const fuchsia_hardware_pwm::PwmChannelInfo> channels =
      metadata.channels().value();
  for (size_t i = 0; i < channels.size(); ++i) {
    const fuchsia_hardware_pwm::PwmChannelInfo& channel = channels[i];
    if (!channel.id().has_value()) {
      fdf::error("Channel {} missing id field", i);
      return zx::error(ZX_ERR_INTERNAL);
    }
    const uint32_t id = channel.id().value();
    if (id > max_pwm_id_) {
      fdf::error("Channel {} has invalid PWM ID {} in metadata: Maximum valid PWM ID is {}", i, id,
                 max_pwm_id_);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    supported_pwm_channels[id] = channel;
  }

  for (uint32_t i = 0; i < mmios.size(); i++) {
    pwms_.push_back(std::make_unique<AmlPwm>(std::move(mmios[i]), supported_pwm_channels[2lu * i],
                                             supported_pwm_channels[(2lu * i) + 1]));
    pwms_.back()->Init();
  }

  zx::result add_service_result = outgoing()->AddService<fuchsia_hardware_pwmimpl::Service>(
      fuchsia_hardware_pwmimpl::Service::InstanceHandler({
          .device = bindings_.CreateHandler(this, driver_dispatcher()->get(),
                                            fidl::kIgnoreBindingClosure),
      }));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add pwm-impl service: {}", add_service_result.status_string());
    return add_service_result.take_error();
  }

  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_pwmimpl::Service>(),
  };
  std::optional metadata_offer = metadata_server_.CreateOffer();
  if (metadata_offer.has_value()) {
    offers.push_back(std::move(metadata_offer.value()));
  }

  const std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwmimpl.Service"),
  };

  zx::result child = AddChild(kChildNodeName, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void AmlPwmDriver::GetConfig(GetConfigRequestView request, fdf::Arena& arena,
                             GetConfigCompleter::Sync& completer) {
  if (request->idx > max_pwm_id_) {
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto result = pwms_[request->idx / 2]->GetConfig(request->idx % 2);
  if (result.is_error()) {
    completer.buffer(arena).ReplyError(result.error_value());
    return;
  }
  completer.buffer(arena).ReplySuccess(fidl::ToWire(arena, *result));
}

void AmlPwmDriver::SetConfig(SetConfigRequestView request, fdf::Arena& arena,
                             SetConfigCompleter::Sync& completer) {
  if (request->idx > max_pwm_id_) {
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  zx_status_t status =
      pwms_[request->idx / 2]->SetConfig(request->idx % 2, fidl::ToNatural(request->config));
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
  } else {
    completer.buffer(arena).ReplySuccess();
  }
}

void AmlPwmDriver::Enable(EnableRequestView request, fdf::Arena& arena,
                          EnableCompleter::Sync& completer) {
  if (request->idx > max_pwm_id_) {
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  zx_status_t status = pwms_[request->idx / 2]->Enable(request->idx % 2);
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
  } else {
    completer.buffer(arena).ReplySuccess();
  }
}

void AmlPwmDriver::Disable(DisableRequestView request, fdf::Arena& arena,
                           DisableCompleter::Sync& completer) {
  if (request->idx > max_pwm_id_) {
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  zx_status_t status = pwms_[request->idx / 2]->Disable(request->idx % 2);
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
  } else {
    completer.buffer(arena).ReplySuccess();
  }
}

void AmlPwmDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_pwmimpl::PwmImpl> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unexpected PwmImpl FIDL request: 0x{:x}", metadata.method_ordinal);
}

}  // namespace pwm

FUCHSIA_DRIVER_EXPORT2(pwm::AmlPwmDriver);
