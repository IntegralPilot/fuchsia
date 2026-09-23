// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tas27xx.h"

#include <lib/async/cpp/task.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fit/defer.h>
#include <time.h>

#include <algorithm>
#include <memory>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>

namespace audio {

// TODO(https://fxbug.dev/42055135): Add handling for the other formats supported by this hardware.
static const std::vector<uint32_t> kSupportedNumberOfChannels = {2};
static const uint64_t kMaxSupportedChannelsToUseBitmask = 2;
static const std::vector<SampleFormat> kSupportedSampleFormats = {SampleFormat::PCM_SIGNED};
static const std::vector<FrameFormat> kSupportedFrameFormats = {FrameFormat::I2S};
static const std::vector<uint32_t> kSupportedRates = {48'000, 96'000};
static const std::vector<uint8_t> kSupportedBitsPerSlot = {32};
static const std::vector<uint8_t> kSupportedBitsPerSample = {16};
static const audio::DaiSupportedFormats kSupportedDaiFormats = {
    .number_of_channels = kSupportedNumberOfChannels,
    .sample_formats = kSupportedSampleFormats,
    .frame_formats = kSupportedFrameFormats,
    .frame_rates = kSupportedRates,
    .bits_per_slot = kSupportedBitsPerSlot,
    .bits_per_sample = kSupportedBitsPerSample,
};

bool Tas27xx::InErrorState() {
  uint8_t pwr_ctl;
  constexpr uint8_t kPwrCtlModeMask = 0x3;
  constexpr uint8_t kPwrCtlModeShutdown = 0x2;
  zx_status_t status = ReadReg(PWR_CTL, &pwr_ctl);
  return started_ && status == ZX_OK && (pwr_ctl & kPwrCtlModeMask) == kPwrCtlModeShutdown;
}

Tas27xx::Tas27xx(zx_device_t* device, ddk::I2cChannel i2c,
                 fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> fault_gpio, bool vsense, bool isense)
    : SimpleCodecServer(device),
      i2c_(std::move(i2c)),
      fault_gpio_(std::move(fault_gpio)),
      ena_vsens_(vsense),
      ena_isens_(isense) {
  zx::result metadata = ddk::GetEncodedMetadata<fuchsia_hardware_ti_metadata::TasMetadata>(
      parent(), DEVICE_METADATA_PRIVATE);
  if (metadata.is_ok()) {
    metadata_ = std::move(metadata.value());
  } else {
    zxlogf(DEBUG, "Failed to get metadata: %s", metadata.status_string());
  }
  driver_inspect_ = inspect().GetRoot().CreateChild("tas27xx");
  resets_count_ = driver_inspect_.CreateUint("resets_count", 0);
}

void Tas27xx::ReportState(State& state, const char* description) {
  int64_t secs = zx::nsec(zx::clock::get_monotonic().get()).to_secs();
  state.seconds = driver_inspect_.CreateInt(std::string("seconds_until_") + description, secs);

  uint8_t ltch0, ltch1, ltch2;
  ReadReg(INT_LTCH0, &ltch0);
  ReadReg(INT_LTCH1, &ltch1);
  ReadReg(INT_LTCH2, &ltch2);

  // Clock error interrupts may happen during a rate change as the codec
  // attempts to auto configure to the tdm bus.
  if (ltch0 & INT_MASK0_TDM_CLOCK_ERROR) {
    zxlogf(INFO, "tas27xx: TDM clock disrupted (may be due to rate change)");
  }
  // While these are logged as errors, the amp will enter a shutdown mode
  // until the condition is remedied, then the output will ramp back on.
  if (ltch0 & INT_MASK0_OVER_CURRENT_ERROR) {
    zxlogf(ERROR, "tas27xx: Over current error");
  }
  if (ltch0 & INT_MASK0_OVER_TEMP_ERROR) {
    zxlogf(ERROR, "tas27xx: Over temperature error");
  }
  state.latched_interrupt_state = driver_inspect_.CreateUint(
      std::string("after_") + description + "_latched_interrupt_state",
      (static_cast<uint64_t>(ltch0) << 0) | (static_cast<uint64_t>(ltch1) << 8) |
          (static_cast<uint64_t>(ltch2) << 16));

  float temperature = 0.f;
  zx_status_t status = GetTemperature(&temperature);
  if (status == ZX_OK) {
    state.temperature = driver_inspect_.CreateInt(std::string("after_") + description + "_mcelsius",
                                                  static_cast<int64_t>(temperature * 1'000.f));
  }
  float voltage = 0.f;
  status = GetVbat(&voltage);
  if (status == ZX_OK) {
    state.voltage = driver_inspect_.CreateUint(std::string("after_") + description + "_mvolt",
                                               static_cast<uint64_t>(voltage * 1'000.f));
  }
}

void Tas27xx::PeriodicStateCheck() {
  if (InErrorState()) {
    zxlogf(ERROR, "codec in error state");
    if (errors_count_ == 0) {
      int64_t secs = zx::nsec(zx::clock::get_monotonic().get()).to_secs();
      first_error_secs_ = driver_inspect_.CreateInt("seconds_until_first_error", secs);
    }
    errors_count_++;

    ReportState(state_after_error_, "error");

    constexpr uint32_t kMaxRetries = 8;  // We don't want to reset forever.
    if (errors_count_ <= kMaxRetries) {
      resets_count_.Add(1);
      Reset();
      SetGainStateInternal(gain_state_);
      if (format_.has_value()) {
        [[maybe_unused]] auto format_info = SetDaiFormatInternal(*format_);
      }
      Start();
    }
  } else {
    ReportState(state_after_timer_, "timer");
  }

  // It is safe to capture "this" here, the dispatcher's loop is guaranteed to be shutdown
  // before this object is destroyed.
  async::PostDelayedTask(dispatcher(), [this]() { PeriodicStateCheck(); }, zx::sec(20));
}

zx_status_t Tas27xx::GetTemperature(float* temperature) {
  uint8_t reg;
  zx_status_t status = ReadReg(TEMP_MSB, &reg);
  if (status != ZX_OK) {
    return status;
  }
  // Slope and offset from TAS2770 Datasheet
  *temperature = static_cast<float>(-93.0 + (reg << 4) * 0.0625);
  status = ReadReg(TEMP_LSB, &reg);
  if (status != ZX_OK) {
    return status;
  }
  *temperature = *temperature + static_cast<float>((reg >> 4) * 0.0625);
  constexpr float kMinimumTemperature = -93.f;
  if (*temperature == kMinimumTemperature) {
    return ZX_ERR_SHOULD_WAIT;  // Not available.
  }
  return status;
}

zx_status_t Tas27xx::GetVbat(float* voltage) {
  uint8_t reg;
  zx_status_t status = ReadReg(VBAT_MSB, &reg);
  if (status != ZX_OK) {
    return status;
  }
  // Slope and offset from TAS2770 Datasheet
  *voltage = static_cast<float>((reg << 4) * 0.0039);
  status = ReadReg(VBAT_LSB, &reg);
  if (status != ZX_OK) {
    return status;
  }
  *voltage = *voltage + static_cast<float>((reg >> 4) * 0.0039);
  if (*voltage == 0.f) {
    return ZX_ERR_SHOULD_WAIT;  // Not available.
  }
  return status;
}

// If started, puts codec in active but muted/unmuted state.
// If stopped, puts codec in shutdown state.
// Sets I and V sense features to proper state
zx_status_t Tas27xx::UpdatePowerControl() {
  if (started_) {
    return WriteReg(PWR_CTL, static_cast<uint8_t>(((!ena_isens_) << 3) | ((!ena_vsens_) << 2) |
                                                  (static_cast<uint8_t>(gain_state_.muted) << 0)));
  } else {
    constexpr uint8_t kPwrCtlModeShutdown = 0x2;
    return WriteReg(PWR_CTL,
                    static_cast<uint8_t>((1 << 3) | (1 << 2) | (kPwrCtlModeShutdown << 0)));
  }
}

// Puts in shutdown state (clocks must be active or TDM error will trigger)
// Sets I and V sense features to proper state
zx_status_t Tas27xx::Stop() {
  started_ = false;
  return UpdatePowerControl();
}

// Puts in active state (clocks must be active or TDM error will trigger)
// Sets I and V sense features to proper state
zx_status_t Tas27xx::Start() {
  started_ = true;
  return UpdatePowerControl();
}

GainFormat Tas27xx::GetGainFormat() {
  return {
      .min_gain = kMinGain,
      .max_gain = kMaxGain,
      .gain_step = kGainStep,
      .can_mute = true,
      .can_agc = false,
  };
}

GainState Tas27xx::GetGainState() { return gain_state_; }

void Tas27xx::SetGainState(GainState gain_state) {
  gain_state_ = gain_state;
  SetGainStateInternal(gain_state);
}

void Tas27xx::SetGainStateInternal(GainState gain_state) {
  gain_state.gain = std::clamp(gain_state.gain, kMinGain, kMaxGain);
  uint8_t gain_reg = static_cast<uint8_t>(-gain_state.gain / kGainStep);
  WriteReg(PB_CFG2, gain_reg);
  if (gain_state.agc_enabled) {
    zxlogf(ERROR, "tas27xx: AGC enable not supported");
    gain_state.agc_enabled = false;
  }
  UpdatePowerControl();
}

bool Tas27xx::ValidGain(float gain) { return (gain <= kMaxGain) && (gain >= kMinGain); }

zx_status_t Tas27xx::SetRate(uint32_t rate) {
  // Note: autorate is enabled below, so changing the codec rate is not strictly required.
  // bit[5]   - rate ramp, 0=48kHz, 1=44.1kHz
  // bit[4]   - auto rate, 0=enable
  // bit[3:1] - samp rate, 3=48kHz, 4=96kHz
  // bit[0]   - fsync edge, 0 = rising edge, 1 = falling edge
  return WriteReg(
      TDM_CFG0, static_cast<uint8_t>((0 << 5) | (0 << 4) | (((rate == 96000) ? 0x04 : 0x03) << 1) |
                                     (1 << 0)));
}

zx_status_t Tas27xx::SetTdmSlots(uint64_t channels_to_use_bitmask) {
  // kMaxSupportedChannelsToUseBitmask is 0x02: only 1 of the 2 available channels can be enabled.
  // bit[5:4] - RX_SCFG, 01b Mono, Right channel or 10b = Mono, Left channel.
  // bit[3:2] - RX_WLEN, 00b = 16-bits word length
  // bit[0:1] - RX_SLEN, 10b = 32-bit slot length
  channels_to_use_bitmask_ = channels_to_use_bitmask;
  uint8_t rx_scfg = static_cast<uint8_t>(channels_to_use_bitmask_);
  return WriteReg(TDM_CFG2, (rx_scfg << 4) | (0x00 << 2) | 0x02);
}

void Tas27xx::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                        const zx_packet_interrupt_t* interrupt) {
  if (status == ZX_OK) {  // We only report state on good IRQ callbacks.
    ReportState(state_after_interrupt_, "interrupt");
  }
  irq_.ack();
}

zx::result<DriverIds> Tas27xx::Initialize() {
  fidl::Arena arena;
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                              .Build();
  fidl::WireResult config_result = fault_gpio_->ConfigureInterrupt(interrupt_config);
  if (!config_result.ok()) {
    zxlogf(ERROR, "Failed to send ConfigureInterrupt request to fault gpio: %s",
           config_result.status_string());
    return zx::error(config_result.status());
  }
  if (config_result->is_error()) {
    zxlogf(ERROR, "Failed to configure interrupt on fault gpio: %s",
           zx_status_get_string(config_result->error_value()));
    return config_result->take_error();
  }

  fidl::WireResult result = fault_gpio_->GetInterrupt({});
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send GetInterrupt request to fault gpio: %s", result.status_string());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to get interrupt from fault gpio: %s",
           zx_status_get_string(result->error_value()));
    return result->take_error();
  }
  irq_ = std::move(result.value()->interrupt);
  irq_handler_.set_object(irq_.get());
  irq_handler_.Begin(dispatcher());

  // Start a periodic state check.
  // It is safe to capture "this" here, the dispatcher's loop is guaranteed to be shutdown
  // before this object is destroyed.
  async::PostDelayedTask(dispatcher(), [this]() { PeriodicStateCheck(); }, zx::sec(20));

  return zx::ok(DriverIds{
      .vendor_id = PDEV_VID_TI,
      .device_id = PDEV_DID_TI_TAS2770,
  });
}

zx_status_t Tas27xx::Reinitialize() {
  auto status = Stop();
  if (status != ZX_OK) {
    return status;
  }

  // bit[5:2] - SBCLK_FS_RATIO - frame sync to sclk ratio
  //             64 for two channel i2s (32 bits per channel)
  // bit[1:0] - AUTO_CLK - 1=manual, 0=auto
  status = WriteReg(CLOCK_CFG, (SBCLK_FS_RATIO_64 << 2));
  if (status != ZX_OK) {
    return status;
  }

  // Set initial configuration of rate
  status = SetRate(kSupportedRates[0]);
  if (status != ZX_OK) {
    return status;
  }

  status = SetTdmSlots(channels_to_use_bitmask_);
  if (status != ZX_OK) {
    return status;
  }

  // bit[4] - 0=transmit 0 on unusued slots
  // bit[3:1] tx offset -1 per i2s
  // bit[0]   tx_edge, 0 = clock out on falling edge of sbclk
  status = WriteReg(TDM_CFG4, (1 << 1) | (0 << 0));
  if (status != ZX_OK) {
    return status;
  }

  // bit[6] - 1 = Enable vsense transmit on sdout
  // bit[5:0] - tdm bus time slot for vsense
  //            all tx slots are 8-bits wide
  //            slot 4 will align with second i2s channel
  status = WriteReg(TDM_CFG5, (0x01 << 6) | 0x04);
  if (status != ZX_OK) {
    return status;
  }

  // bit[6] - 1 = Enable isense transmit on sdout
  // bit[5:0] - tdm bus time slot for isense
  //            all tx slots are 8-bits wide
  status = WriteReg(TDM_CFG6, (0x01 << 6) | 0x00);
  if (status != ZX_OK) {
    return status;
  }

  // Read latched interrupt registers to clear
  uint8_t temp;
  ReadReg(INT_LTCH0, &temp);
  ReadReg(INT_LTCH1, &temp);
  ReadReg(INT_LTCH2, &temp);

  // Set interrupt masks
  status = WriteReg(INT_MASK0, ~(INT_MASK0_TDM_CLOCK_ERROR | INT_MASK0_OVER_CURRENT_ERROR |
                                 INT_MASK0_OVER_TEMP_ERROR));
  if (status != ZX_OK) {
    return status;
  }

  status = WriteReg(INT_MASK1, 0xff);
  if (status != ZX_OK) {
    return status;
  }
  // Interupt on any unmasked latched interrupts
  status = WriteReg(INT_CFG, 0x01);
  if (status != ZX_OK) {
    return status;
  }
  SetGainStateInternal(kDefaultGainState);
  return ZX_OK;
}

zx_status_t Tas27xx::Reset() {
  // Will be in software shutdown state after call.
  zx_status_t status = WriteReg(SW_RESET, 0x01);
  if (status != ZX_OK) {
    DelayMs(2);
    return status;
  }
  for (const fuchsia_hardware_ti_metadata::Register& register_setting :
       metadata_.init_sequence1()) {
    auto status = WriteReg(register_setting.address(), register_setting.value());
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to write I2C register 0x%02X", register_setting.address());
      return status;
    }
  }

  DelayMs(2);
  // Run the second init sequence from metadata if available.
  for (const fuchsia_hardware_ti_metadata::Register& register_setting :
       metadata_.init_sequence2()) {
    auto status = WriteReg(register_setting.address(), register_setting.value());
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to write I2C register 0x%02X", register_setting.address());
      return status;
    }
  }
  return Reinitialize();
}

Info Tas27xx::GetInfo() {
  return {
      .manufacturer = "Texas Instruments",
      .product_name = "TAS2770",
  };
}

zx_status_t Tas27xx::Shutdown() {
  irq_handler_.Cancel();
  irq_.destroy();
  return ZX_OK;
}

DaiSupportedFormats Tas27xx::GetDaiFormats() { return kSupportedDaiFormats; }

zx::result<CodecFormatInfo> Tas27xx::SetDaiFormat(const DaiFormat& format) {
  auto result = SetDaiFormatInternal(format);
  if (result.is_ok()) {
    format_.emplace(format);
  }
  return result;
}

template <typename X>
bool contains(std::vector<X> vec, X val) {
  return std::any_of(vec.cbegin(), vec.cend(), [val](X v) { return (val == v); });
}

zx_status_t CheckFormatSupported(const DaiFormat& format) {
  if (!contains(kSupportedNumberOfChannels, format.number_of_channels)) {
    zxlogf(ERROR, "tas27xx: number of channels not supported %u", format.number_of_channels);
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (format.channels_to_use_bitmask > kMaxSupportedChannelsToUseBitmask) {
    zxlogf(ERROR, "tas27xx: channels to use not supported %lu", format.channels_to_use_bitmask);
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!contains(kSupportedSampleFormats, format.sample_format)) {
    zxlogf(ERROR, "tas27xx: sample format not supported");
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!contains(kSupportedFrameFormats, format.frame_format)) {
    zxlogf(ERROR, "tas27xx: frame format not supported");
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!contains(kSupportedRates, format.frame_rate)) {
    zxlogf(ERROR, "tas27xx: rate not supported %u", format.frame_rate);
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!contains(kSupportedBitsPerSlot, format.bits_per_slot)) {
    zxlogf(ERROR, "tas27xx: bits per slot not supported %u",
           static_cast<uint16_t>(format.bits_per_slot));
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!contains(kSupportedBitsPerSample, format.bits_per_sample)) {
    zxlogf(ERROR, "tas27xx: bits per sample not supported %u",
           static_cast<uint16_t>(format.bits_per_sample));
    return ZX_ERR_NOT_SUPPORTED;
  }
  return ZX_OK;
}

zx::result<CodecFormatInfo> Tas27xx::SetDaiFormatInternal(const DaiFormat& format) {
  zx_status_t status = CheckFormatSupported(format);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = SetRate(format.frame_rate);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = SetTdmSlots(format.channels_to_use_bitmask);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  CodecFormatInfo info = {};

  // Datasheet states "Turn on time from release of SW shutdown" with "Volume Ramping" as "5.3ms".
  constexpr int64_t turn_on_delay_usec = 5'300;
  info.set_turn_on_delay(zx::usec(turn_on_delay_usec).get());

  // Datasheet states "Turn off time from assertion of SW shutdown to amp Hi-Z" with
  // "Volume Ramping" as "4.7ms".
  constexpr int64_t turn_off_delay_usec = 4'700;
  info.set_turn_off_delay(zx::usec(turn_off_delay_usec).get());

  return zx::ok(std::move(info));
}

zx_status_t Tas27xx::WriteReg(uint8_t reg, uint8_t value) {
  uint8_t write_buffer[2];
  write_buffer[0] = reg;
  write_buffer[1] = value;
// #define TRACE_I2C
#ifdef TRACE_I2C
  printf("Writing register 0x%02X to value 0x%02X\n", reg, value);
#endif
  constexpr uint8_t kNumberOfRetries = 2;
  constexpr zx::duration kRetryDelay = zx::msec(1);
  auto ret =
      i2c_.WriteSyncRetries(write_buffer, std::size(write_buffer), kNumberOfRetries, kRetryDelay);
  if (ret.status != ZX_OK) {
    zxlogf(ERROR, "tas27xx: I2C write reg 0x%02X error %d, %d retries", reg, ret.status,
           ret.retries);
  }
  return ret.status;
}

zx_status_t Tas27xx::ReadReg(uint8_t reg, uint8_t* value) {
  constexpr uint8_t kNumberOfRetries = 2;
  constexpr zx::duration kRetryDelay = zx::msec(1);
  auto ret = i2c_.WriteReadSyncRetries(&reg, 1, value, 1, kNumberOfRetries, kRetryDelay);
  if (ret.status != ZX_OK) {
    zxlogf(ERROR, "tas27xx: I2C read reg 0x%02X error %d, %d retries", reg, ret.status,
           ret.retries);
  }
#ifdef TRACE_I2C
  printf("Read register 0x%02X, value %02X\n", reg, *value);
#endif
  return ret.status;
}

zx_status_t tas27xx_bind(void* ctx, zx_device_t* parent) {
  ddk::I2cChannel i2c(parent, "i2c");
  if (!i2c.is_valid()) {
    zxlogf(ERROR, "tas27xx: Could not get i2c protocol");
    return ZX_ERR_NO_RESOURCES;
  }

  zx::result gpio =
      ddk::Device<void>::DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
          parent, "gpio-fault");
  if (gpio.is_error()) {
    zxlogf(ERROR, "tas27xx: Failed to get gpio protocol: %s", gpio.status_string());
    return ZX_ERR_NOT_SUPPORTED;
  }

  return SimpleCodecServer::CreateAndAddToDdk<Tas27xx>(parent, std::move(i2c),
                                                       std::move(gpio.value()), false, false);
}

static zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = tas27xx_bind;
  return ops;
}();

}  // namespace audio

ZIRCON_DRIVER(ti_tas27xx, audio::driver_ops, "zircon", "0.1");
