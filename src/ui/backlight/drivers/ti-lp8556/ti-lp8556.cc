// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ti-lp8556.h"

#include <endian.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <math.h>

#include <algorithm>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <pretty/hexdump.h>

namespace ti {

// Panel type ID provided by the bootloader to Zircon.
//
// Values must be kept in sync with the bootloader implementation.
enum class BootloaderPanelType : uint32_t {
  kKdFiti9364 = 1,
  kBoeFiti9364 = 2,
  // 3 was for kFiti9364
  kKdFiti9365 = 4,
  kBoeFiti9365 = 5,
  // 6 was for kSit7703.
};

TiLp8556::TiLp8556() : fdf::DriverBase2(kDriverName) {}

display::PanelType ToDisplayPanelType(BootloaderPanelType bootloader_panel_type) {
  switch (bootloader_panel_type) {
    case BootloaderPanelType::kKdFiti9364:
      return display::PanelType::kKdKd070d82FitipowerJd9364;
    case BootloaderPanelType::kBoeFiti9364:
      return display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson;
    case BootloaderPanelType::kKdFiti9365:
      return display::PanelType::kKdKd070d82FitipowerJd9365;
    case BootloaderPanelType::kBoeFiti9365:
      return display::PanelType::kBoeTv070wsmFitipowerJd9365;
  }
  fdf::error("Unknown panel type {}", static_cast<uint32_t>(bootloader_panel_type));
  return display::PanelType::kUnknown;
}

// Refer to <internal>/vendor/amlogic/video-common/ambient_temp/lp8556.cc
// Lookup tables containing the slope and y-intercept for a linear equation used
// to fit the (power / |brightness_to_current_scalar_|) per vendor for
// brightness levels below |kMinTableBrightness|. The power can be calculated
// from these scalars by:
// (slope * brightness + intercept) * |brightness_to_current_scalar_|.
constexpr std::array<double,
                     static_cast<std::size_t>(TiLp8556::PanelType::kNumTypes)>
    kLowBrightnessSlopeTable = {
        22.4,  // PanelType::kBoe
        22.2,  // PanelType::kKd
        22.2,  // PanelType::kUnknown
};
constexpr std::array<double,
                     static_cast<std::size_t>(TiLp8556::PanelType::kNumTypes)>
    kLowBrightnessInterceptTable = {
        1236.0,  // PanelType::kBoe
        1319.0,  // PanelType::kKd
        1329.0,  // PanelType::kUnknown
};

// Lookup tables for backlight driver voltage as a function of the backlight
// brightness. The index for each sub-table corresponds to a PanelType, and
// allows for the backlight voltage to vary with panel vendor. Starting from a
// brightness level of |kMinTableBrightness|, each index of each sub-table
// corresponds to a jump of |kBrightnessStep| in brightness up to the maximum
// value of |kMaxBrightnessSetting|.
constexpr std::array<std::array<double, kTableSize>,
                     static_cast<std::size_t>(TiLp8556::PanelType::kNumTypes)>
    kVoltageTable = {{
        // PanelType::kBoe
        {19.80, 19.80, 19.80, 19.80, 19.90, 20.00, 20.10, 20.20, 20.30, 20.40, 20.50, 20.53, 20.53,
         20.53, 20.53, 20.53},
        // PanelType::kKd
        {19.67, 19.67, 19.67, 19.67, 19.77, 19.93, 20.03, 20.13, 20.20, 20.27, 20.37, 20.37, 20.37,
         20.37, 20.37, 20.37},
        // PanelType:kUnknown
        {19.72, 19.72, 19.72, 19.72, 19.82, 19.94, 20.04, 20.14, 20.23, 20.31, 20.39, 20.40, 20.40,
         20.40, 20.40, 20.40},
    }};

// Lookup table for backlight driver efficiency as a function of the backlight
// brightness. Starting from a brightness level of |kMinTableBrightness|, each
// index of the table corresponds to a jump of |kBrightnessStep| in brightness
// up to the maximum value of |kMaxBrightnessSetting|.
constexpr std::array<double, kTableSize> kEfficiencyTable = {
    0.6680, 0.7784, 0.8240, 0.8484, 0.8634, 0.8723, 0.8807, 0.8860,
    0.8889, 0.8915, 0.8953, 0.8983, 0.9003, 0.9034, 0.9049, 0.9060};

// The max current value in the table is determined by the value of the three
// max current bits within the TiLp8556 CFG1 register. The value of these bits can
// be obtained from the max_current sysfs node exposed by the driver. The
// current values in the table are expressed in mA.
constexpr std::array<double, 8> kMaxCurrentTable = {5.0, 10.0, 15.0, 20.0, 23.0, 25.0, 30.0, 50.0};

zx::result<> TiLp8556::GetBacklightState(bool* power, double* brightness) const {
  *power = power_;
  *brightness = brightness_;
  return zx::ok();
}

zx::result<> TiLp8556::SetBacklightState(bool power, double brightness) {
  brightness = std::max(brightness, 0.0);
  brightness = std::min(brightness, 1.0);
  uint16_t brightness_reg_value = static_cast<uint16_t>(ceil(brightness * kBrightnessRegMaxValue));
  if (brightness != brightness_) {
    // LSB should be updated before MSB. Writing to MSB triggers the brightness change.
    std::array<uint8_t, 2> write_data{
        kBacklightBrightnessLsbReg,
        static_cast<uint8_t>(brightness_reg_value & kBrightnessLsbMask)};
    if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
      fdf::error("Failed to set brightness LSB register: {}", result);
      return result.take_error();
    }

    zx::result msb_reg_value = ReadI2cByteSync(kBacklightBrightnessMsbReg);
    if (msb_reg_value.is_error()) {
      fdf::error("Failed to get brightness MSB register: {}", msb_reg_value);
      return msb_reg_value.take_error();
    }

    // The low 4-bits contain the brightness MSB. Keep the remaining bits unchanged.]
    uint8_t new_msg_reg_value = msb_reg_value.value() & ~kBrightnessMsbByteMask;
    new_msg_reg_value |=
        (static_cast<uint8_t>((brightness_reg_value & kBrightnessMsbMask) >> kBrightnessMsbShift));

    write_data[0] = kBacklightBrightnessMsbReg;
    write_data[1] = new_msg_reg_value;
    if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
      fdf::error("Failed to set brightness MSB register: {}", result);
      return result.take_error();
    }

    auto persistent_brightness = BrightnessStickyReg::Get().ReadFrom(&mmio_.value());
    persistent_brightness.set_brightness(brightness_reg_value & kBrightnessRegMask);
    persistent_brightness.set_is_valid(1);
    persistent_brightness.WriteTo(&mmio_.value());
  }

  if (power != power_) {
    std::array<uint8_t, 2> write_data{
        kDeviceControlReg,
        static_cast<uint8_t>(kDeviceControlDefaultValue | (power ? kBacklightOn : 0))};
    if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
      fdf::error("Failed to set device control register: {}", result);
      return result.take_error();
    }

    if (power) {
      for (const fuchsia_hardware_ti_metadata::Register& reg : registers_) {
        const std::array<uint8_t, 2> write_data = {reg.address(), reg.value()};
        if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
          fdf::error("Failed to set register {:#02x}: {}", reg.address(), result);
          return result.take_error();
        }
      }

      write_data[0] = kCfg2Reg;
      write_data[1] = cfg2_;
      if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
        fdf::error("Failed to set cfg2 register: {}", result);
        return result.take_error();
        ;
      }
    }
  }

  // update internal values
  power_ = power;
  brightness_ = brightness;
  power_property_.Set(power_);
  brightness_property_.Set(brightness_);
  backlight_power_ = GetBacklightPower(brightness_reg_value);
  power_watts_property_.Set(backlight_power_);

  return zx::ok();
}

void TiLp8556::GetStateNormalized(GetStateNormalizedCompleter::Sync& completer) {
  fuchsia_hardware_backlight::wire::State state = {};
  zx::result result = GetBacklightState(&state.backlight_on, &state.brightness);
  if (result.is_ok()) {
    completer.ReplySuccess(state);
  } else {
    completer.ReplyError(result.status_value());
  }
}

void TiLp8556::SetStateNormalized(SetStateNormalizedRequestView request,
                                  SetStateNormalizedCompleter::Sync& completer) {
  zx::result result = SetBacklightState(request->state.backlight_on, request->state.brightness);
  if (result.is_ok()) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(result.status_value());
  }
}

void TiLp8556::GetStateAbsolute(GetStateAbsoluteCompleter::Sync& completer) {
  if (!max_absolute_brightness_nits_.has_value()) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  if (scale_ != calibrated_scale_) {
    fdf::error("Can't get absolute state with non-calibrated current scale");
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  fuchsia_hardware_backlight::wire::State state = {};
  zx::result result = GetBacklightState(&state.backlight_on, &state.brightness);
  if (result.is_ok()) {
    state.brightness *= max_absolute_brightness_nits_.value();
    completer.ReplySuccess(state);
  } else {
    completer.ReplyError(result.status_value());
  }
}

void TiLp8556::SetStateAbsolute(SetStateAbsoluteRequestView request,
                                SetStateAbsoluteCompleter::Sync& completer) {
  if (!max_absolute_brightness_nits_.has_value()) {
    fdf::error("Failed to set state: Device does not have a max brightness");
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  // Restore the calibrated current scale that the bootloader set. This and the maximum brightness
  // are the only values we have that can be used to set the absolute brightness in nits.
  if (zx::result result = SetCurrentScale(calibrated_scale_); result.is_error()) {
    completer.ReplyError(result.status_value());
    return;
  }

  zx::result result =
      SetBacklightState(request->state.backlight_on,
                        request->state.brightness / max_absolute_brightness_nits_.value());
  if (result.is_ok()) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(result.status_value());
  }
}

void TiLp8556::GetMaxAbsoluteBrightness(GetMaxAbsoluteBrightnessCompleter::Sync& completer) {
  if (max_absolute_brightness_nits_.has_value()) {
    completer.ReplySuccess(max_absolute_brightness_nits_.value());
  } else {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
}

void TiLp8556::GetPowerWatts(GetPowerWattsCompleter::Sync& completer) {
  // Only supported on Nelson for now.
  if (board_pid_ == PDEV_PID_NELSON) {
    completer.ReplySuccess(static_cast<float>(backlight_power_));
  } else {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
}

void TiLp8556::GetVoltageVolts(GetVoltageVoltsCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void TiLp8556::GetSensorName(GetSensorNameCompleter::Sync& completer) {
  completer.Reply(fidl::StringView::FromExternal(kPowerSensorName));
}

zx::result<display::PanelType> TiLp8556::GetDisplayPanelInfo(fdf::Namespace* incoming) {
  zx::result panel_type_result =
      compat::GetMetadata<display::PanelType>(incoming, DEVICE_METADATA_DISPLAY_PANEL_TYPE, "pdev");
  if (panel_type_result.is_ok()) {
    return zx::ok(*panel_type_result.value());
  }

  // If the display panel info is not available from the panel type metadata,
  // try to get it from the bootloader metadata.
  fdf::info(
      "Trying bootloader metadata: Failed to get display panel info from panel type metadata: {}",
      panel_type_result);

  zx::result bootloader_metadata_result =
      compat::GetMetadata<display::PanelType>(incoming, DEVICE_METADATA_BOARD_PRIVATE, "pdev");
  if (bootloader_metadata_result.is_ok()) {
    return zx::ok(*bootloader_metadata_result.value());
  }
  if (bootloader_metadata_result.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Failed to get bootloader metadata: {}", bootloader_metadata_result);
    return bootloader_metadata_result.take_error();
  }

  // If the panel type is not available to the backlight driver (on Astro and
  // Sherlock), or the panel type is unknown to the board driver (on Nelson),
  // we use `display::PanelType::kUnknown` to fall back to the default backlight power
  // configurations.

  fdf::info("Failed to get bootloader metadata: {}", bootloader_metadata_result);
  return zx::ok(display::PanelType::kUnknown);
}

zx::result<> TiLp8556::Start(fdf::DriverContext context) {
  auto incoming = context.take_incoming();
  root_ = inspector_.GetRoot().CreateChild("ti-lp8556");

  // Obtain I2C protocol needed to control backlight
  zx::result i2c = incoming->Connect<fuchsia_hardware_i2c::Service::Device>("i2c");
  if (i2c.is_error()) {
    fdf::error("Failed to connect to i2c: {}", i2c);
    return i2c.take_error();
  }
  i2c_ = i2c::I2cChannel{std::move(i2c.value())};

  zx::result pdev_client_end =
      incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client_end.value()));

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_ti_metadata::Lp8556Metadata>();
  // Supplying this metadata is optional.
  if (metadata.is_ok()) {
    if (!metadata->backlight_max_brightness().has_value()) {
      fdf::error("Metadata missing `backlight_max_brightness` field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (!metadata->registers().has_value()) {
      fdf::error("Metadata missing `registers` field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    registers_ = std::move(metadata->registers().value());

    SetMaxAbsoluteBrightnessNits(metadata->backlight_max_brightness().value());

    for (const fuchsia_hardware_ti_metadata::Register& reg : registers_) {
      const std::array<uint8_t, 2> write_data = {reg.address(), reg.value()};
      if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
        fdf::error("Failed to set register {:#02x}: {}", reg.address(), result);
        return result.take_error();
      }
    }
  }

  zx::result<display::PanelType> panel_type_result = GetDisplayPanelInfo(incoming.get());
  if (panel_type_result.is_error()) {
    fdf::error("Failed to get display panel info: {}", panel_type_result);
    return panel_type_result.take_error();
  }
  panel_type_ = panel_type_result.value();

  zx::result board_info = pdev.GetBoardInfo();
  if (board_info.is_ok()) {
    board_pid_ = board_info->pid;
  }

  zx::result mmio = pdev.MapMmio(0);
  if (mmio.is_error()) {
    fdf::error("Failed to map mmio: {}", mmio);
    return mmio.take_error();
  }
  mmio_.emplace(std::move(mmio.value()));

  auto persistent_brightness = BrightnessStickyReg::Get().ReadFrom(&mmio_.value());
  if (persistent_brightness.is_valid()) {
    persistent_brightness_property_ =
        root_.CreateUint("persistent_brightness", persistent_brightness.brightness());
  }

  if (zx::result result = ReadInitialState(); result.is_error()) {
    return result.take_error();
  }

  brightness_property_ = root_.CreateDouble("brightness", brightness_);
  scale_property_ = root_.CreateUint("scale", scale_);
  calibrated_scale_property_ = root_.CreateUint("calibrated_scale", calibrated_scale_);
  power_property_ = root_.CreateBool("power", power_);
  power_watts_property_ = root_.CreateDouble("power_watts", backlight_power_);

  board_pid_property_ = root_.CreateUint("board_pid", board_pid_);
  panel_id_property_ = root_.CreateUint("panel_id", static_cast<uint32_t>(panel_type_));
  panel_type_property_ = root_.CreateUint("panel_type", static_cast<uint32_t>(GetPanelType()));

  fuchsia_hardware_backlight::Service::InstanceHandler backlight_instance_handler{
      {.backlight =
           backlight_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure)}};
  if (zx::result result = outgoing()->AddService<fuchsia_hardware_backlight::Service>(
          std::move(backlight_instance_handler));
      result.is_error()) {
    fdf::error("Failed to add backlight service: {}", result);
    return result.take_error();
  }

  fuchsia_hardware_power_sensor::Service::InstanceHandler power_sensor_instance_handler{
      {.device =
           power_sensor_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure)}};
  if (zx::result result = outgoing()->AddService<fuchsia_hardware_power_sensor::Service>(
          std::move(power_sensor_instance_handler));
      result.is_error()) {
    fdf::error("Failed to add power sensor service: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> TiLp8556::SetCurrentScale(uint16_t scale) {
  scale &= kBrightnessRegMask;

  if (scale == scale_) {
    return zx::ok();
  }

  zx::result msb_reg_value = ReadI2cByteSync(kCfgReg);
  if (msb_reg_value.is_error()) {
    fdf::error("Failed to get current scale register: {}", msb_reg_value);
    return msb_reg_value.take_error();
  }

  uint8_t new_msg_reg_value = msb_reg_value.value() & ~kBrightnessMsbByteMask;
  new_msg_reg_value = static_cast<uint8_t>(new_msg_reg_value | (scale >> kBrightnessMsbShift));

  std::array<uint8_t, 3> write_data{
      kCurrentLsbReg,
      static_cast<uint8_t>(scale & kBrightnessLsbMask),
      new_msg_reg_value,
  };
  if (zx::result result = i2c_.WriteSync(write_data); result.is_error()) {
    fdf::error("Failed to set current scale register: {}", result);
    return result.take_error();
  }

  scale_ = scale;
  scale_property_.Set(scale);

  return zx::ok();
}

double TiLp8556::GetBacklightPower(double backlight_brightness) const {
  if (board_pid_ != PDEV_PID_NELSON) {
    return 0;
  }

  // For brightness values less than |kMinTableBrightness|, estimate the power
  // on a per-vendor basis from a linear equation derived from validation data.
  if (backlight_brightness < kMinTableBrightness) {
    std::size_t panel_type_index = static_cast<std::size_t>(GetPanelType());
    double slope = kLowBrightnessSlopeTable[panel_type_index];
    double intercept = kLowBrightnessInterceptTable[panel_type_index];
    return (slope * backlight_brightness + intercept) * GetBrightnesstoCurrentScalar();
  }

  // For brightness values in the range [|kMinTableBrightness|,
  // |kMaxBrightnessSetting|], use the voltage and efficiency lookup tables
  // derived from validation data to estimate the power.
  double backlight_voltage = GetBacklightVoltage(backlight_brightness, GetPanelType());
  double current_amp = GetBrightnesstoCurrentScalar() * backlight_brightness;
  double driver_efficiency = GetDriverEfficiency(backlight_brightness);
  return backlight_voltage * current_amp / driver_efficiency;
}

double TiLp8556::GetBrightnesstoCurrentScalar() const {
  double max_current_amp = max_current_ / kMilliampPerAmp;
  // The setpoint current refers to the backlight current for a single driver
  // channel, assuming that the backlight brightness setting is at its max value
  // of 4095 (100%).
  double setpoint_current_amp = (scale_ / kMaxCurrentSetting) * max_current_amp;
  // The scalar returned is equal to:
  // 6 Driver Channels * Setpoint Current per Channel / Max Brightness Setting
  // When this value is multiplied by the backlight brightness setting, it
  // yields the backlight current in Amps.
  return kNumBacklightDriverChannels * setpoint_current_amp / kMaxBrightnessSetting;
}

double TiLp8556::GetBacklightVoltage(double backlight_brightness, PanelType panel_type) {
  std::size_t panel_type_index = static_cast<std::size_t>(panel_type);

  // Backlight is at max brightness
  if (backlight_brightness == kMaxBrightnessSetting) {
    return kVoltageTable[panel_type_index].back();
  }

  // Backlight is at |kMinTableBrightness|
  if (backlight_brightness == kMinTableBrightness) {
    return kVoltageTable[panel_type_index].front();
  }

  double integral;
  double fractional = modf(backlight_brightness / kBrightnessStep, &integral);
  std::size_t table_index = static_cast<std::size_t>(integral) - 1;
  if (table_index + 1 >= kVoltageTable[panel_type_index].size()) {
    fdf::error("Invalid backlight brightness: {}", backlight_brightness);
    return kVoltageTable[panel_type_index].back();
  }
  double lower_voltage = kVoltageTable[panel_type_index][table_index];
  double upper_voltage = kVoltageTable[panel_type_index][table_index + 1];
  return (upper_voltage - lower_voltage) * fractional + lower_voltage;
}

double TiLp8556::GetDriverEfficiency(double backlight_brightness) {
  // Backlight is at max brightness
  if (backlight_brightness == kMaxBrightnessSetting) {
    return kEfficiencyTable.back();
  }
  // Backlight is at |kMinTableBrightness|
  if (backlight_brightness == kMinTableBrightness) {
    return kEfficiencyTable.front();
  }
  double integral;
  double fractional = modf(backlight_brightness / kBrightnessStep, &integral);
  std::size_t table_index = static_cast<std::size_t>(integral) - 1;
  if (table_index + 1 >= kEfficiencyTable.size()) {
    fdf::error("Invalid backlight brightness: {}", backlight_brightness);
    return kEfficiencyTable.back();
  }
  double lower_efficiency = kEfficiencyTable[table_index];
  double upper_efficiency = kEfficiencyTable[table_index + 1];
  return (upper_efficiency - lower_efficiency) * fractional + lower_efficiency;
}

TiLp8556::PanelType TiLp8556::GetPanelType() const {
  switch (panel_type_) {
    case display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson:
    case display::PanelType::kBoeTv070wsmFitipowerJd9365:
      return TiLp8556::PanelType::kBoe;
    case display::PanelType::kKdKd070d82FitipowerJd9364:
    case display::PanelType::kKdKd070d82FitipowerJd9365:
      return TiLp8556::PanelType::kKd;
    case display::PanelType::kUnknown:
    default:
      return TiLp8556::PanelType::kUnknown;
  }
}

zx::result<> TiLp8556::ReadInitialState() {
  zx::result cfg2 = ReadI2cByteSync(kCfg2Reg);
  if (cfg2.is_error() || cfg2.value() == 0) {
    cfg2_ = kCfg2Default;
  } else {
    cfg2_ = cfg2.value();
  }

  std::array<uint8_t, 2> read_data;
  if (zx::result result = i2c_.ReadSync(kCurrentLsbReg, read_data); result.is_error()) {
    fdf::error("Failed to read current scale value: {}", result);
    return result.take_error();
  }
  scale_ = static_cast<uint16_t>(read_data[0] | (read_data[1] << kBrightnessMsbShift)) &
           kBrightnessRegMask;
  calibrated_scale_ = scale_;

  if (zx::result result = i2c_.ReadSync(kBacklightBrightnessLsbReg, read_data); result.is_ok()) {
    uint16_t brightness_reg;
    memcpy(&brightness_reg, read_data.data(), sizeof(brightness_reg));
    brightness_reg = le16toh(brightness_reg) & kBrightnessRegMask;
    brightness_ = static_cast<double>(brightness_reg) / kBrightnessRegMaxValue;
  } else {
    fdf::error("Failed to read backlight brightness: {}", result);
    brightness_ = 1.0;
    backlight_power_ = 0;
  }

  if (zx::result device_control = ReadI2cByteSync(kDeviceControlReg); device_control.is_ok()) {
    power_ = device_control.value() & kBacklightOn;
  } else {
    fdf::error("Failed to read backlight power: {}", device_control);
    power_ = true;
  }

  // max_absolute_brightness_nits will be initialized in SetMaxAbsoluteBrightnessNits.
  zx::result max_current_idx = ReadI2cByteSync(kCfgReg);
  if (max_current_idx.is_error()) {
    fdf::error("Failed to read max current index: {}", max_current_idx);
    return max_current_idx.take_error();
  }
  max_current_idx.value() = (max_current_idx.value() >> 4) & 0b111;
  max_current_ = kMaxCurrentTable[max_current_idx.value()];

  backlight_power_ = GetBacklightPower(brightness_);

  return zx::ok();
}

zx::result<uint8_t> TiLp8556::ReadI2cByteSync(uint8_t addr) {
  std::array<uint8_t, 1> read_data{0};
  zx::result result = i2c_.ReadSync(addr, read_data);
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(read_data[0]);
}

}  // namespace ti

FUCHSIA_DRIVER_EXPORT2(ti::TiLp8556);
