// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_BACKLIGHT_DRIVERS_TI_LP8556_TI_LP8556_H_
#define SRC_UI_BACKLIGHT_DRIVERS_TI_LP8556_TI_LP8556_H_

#include <fidl/fuchsia.hardware.backlight/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2c/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power.sensor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/inspect/cpp/inspector.h>

#include <optional>

#include <hwreg/bitfields.h>

#include "src/devices/i2c/lib/i2c-channel/i2c-channel.h"

namespace ti {

constexpr uint8_t kBacklightBrightnessLsbReg = 0x10;
constexpr uint8_t kBacklightBrightnessMsbReg = 0x11;
constexpr uint8_t kDeviceControlReg = 0x1;
constexpr uint8_t kCurrentLsbReg = 0xA0;
constexpr uint8_t kCfgReg = 0xA1;
constexpr uint8_t kCfg2Reg = 0xA2;
constexpr uint32_t kAOBrightnessStickyReg = (0x04e << 2);

constexpr uint8_t kBacklightOn = 1;
constexpr uint8_t kDeviceControlDefaultValue = 0x84;
constexpr uint8_t kCfg2Default = 0x30;

constexpr uint16_t kBrightnessRegMask = 0xFFF;
constexpr uint16_t kBrightnessRegMaxValue = kBrightnessRegMask;

constexpr uint16_t kBrightnessMsbShift = 8;
constexpr uint16_t kBrightnessLsbMask = 0xFF;
constexpr uint8_t kBrightnessMsbByteMask = 0xF;
constexpr uint16_t kBrightnessMsbMask = (kBrightnessMsbByteMask << kBrightnessMsbShift);

constexpr int kTableSize = 16;
constexpr int kBrightnessStep = 256;
constexpr float kMinTableBrightness = 256;

constexpr float kMaxCurrentSetting = 4095;
constexpr float kMinBrightnessSetting = 0;
constexpr float kMaxBrightnessSetting = 4095;
constexpr int kNumBacklightDriverChannels = 6;

constexpr int kMilliampPerAmp = 1000;

class BrightnessStickyReg : public hwreg::RegisterBase<BrightnessStickyReg, uint32_t> {
 public:
  // This bit is used to distinguish between a zero register value and an unset value.
  // A zero value indicates that the sticky register has not been set (so a default of 100%
  // brightness will be used by the bootloader).
  // With this bit set, a zero brightness value is encoded as 0x1000 to distinguish it from an unset
  // value.
  DEF_BIT(12, is_valid);
  DEF_FIELD(11, 0, brightness);

  static auto Get() { return hwreg::RegisterAddr<BrightnessStickyReg>(kAOBrightnessStickyReg); }
};

class TiLp8556 : public fdf::DriverBase2,
                 public fidl::WireServer<fuchsia_hardware_backlight::Device>,
                 public fidl::WireServer<fuchsia_hardware_power_sensor::Device> {
 public:
  static constexpr std::string_view kDriverName = "ti_lp8556";

  TiLp8556();

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  zx::result<> GetBacklightState(bool* power, double* brightness) const;
  zx::result<> SetBacklightState(bool power, double brightness);

  double GetDeviceBrightness() const { return brightness_; }
  bool GetDevicePower() const { return power_; }
  uint8_t GetCfg2() const { return cfg2_; }
  void SetMaxAbsoluteBrightnessNits(double brightness_nits) {
    max_absolute_brightness_nits_ = brightness_nits;
    if (max_absolute_brightness_nits_property_) {
      max_absolute_brightness_nits_property_.Set(brightness_nits);
    } else {
      max_absolute_brightness_nits_property_ =
          root_.CreateDouble("max_absolute_brightness_nits", brightness_nits);
    }
  }

  enum class PanelType {
    kBoe = 0,
    kKd = 1,
    kUnknown = 2,
    kNumTypes = 3,
  };

  double GetBacklightPower(double backlight_brightness) const;
  double GetBrightnesstoCurrentScalar() const;
  static double GetBacklightVoltage(double backlight_brightness, PanelType panel_type);
  static double GetDriverEfficiency(double backlight_brightness);
  PanelType GetPanelType() const;

  // fidl::WireServer<fuchsia_hardware_backlight::Device> implementation.
  void GetStateNormalized(GetStateNormalizedCompleter::Sync& completer) override;
  void SetStateNormalized(SetStateNormalizedRequestView request,
                          SetStateNormalizedCompleter::Sync& completer) override;
  // Note: the device is calibrated at the factory to find a normalized brightness scale value that
  // corresponds to a set maximum brightness in nits. GetStateAbsolute() will return an error if
  // the normalized brightness scale is not set to the calibrated value, as there is no universal
  // way to map other scale values to absolute brightness.
  void GetStateAbsolute(GetStateAbsoluteCompleter::Sync& completer) override;
  // Note: this changes the normalized brightness scale back to the calibrated value in order to set
  // the absolute brightness.
  void SetStateAbsolute(SetStateAbsoluteRequestView request,
                        SetStateAbsoluteCompleter::Sync& completer) override;
  void GetMaxAbsoluteBrightness(GetMaxAbsoluteBrightnessCompleter::Sync& completer) override;

  // fidl::WireServer<fuchsia_hardware_power_sensor::Device> implementation.
  void GetPowerWatts(GetPowerWattsCompleter::Sync& completer) override;
  void GetVoltageVolts(GetVoltageVoltsCompleter::Sync& completer) override;
  void GetSensorName(GetSensorNameCompleter::Sync& completer) override;

 protected:
  inspect::Inspector& inspector() { return inspector_; }

 private:
  static constexpr char kPowerSensorName[] = "backlight";

  zx::result<display::PanelType> GetDisplayPanelInfo(fdf::Namespace* incoming);

  // Calls ReadSync() with a read size of 1 and returns the byte that was read.
  zx::result<uint8_t> ReadI2cByteSync(uint8_t addr);

  zx::result<> SetCurrentScale(uint16_t scale);
  zx::result<> ReadInitialState();

  inspect::Node root_;

  // TODO(rashaeqbal): Switch from I2C to PWM in order to support a larger brightness range.
  // Needs a PWM driver.
  i2c::I2cChannel i2c_;
  std::optional<fdf::MmioBuffer> mmio_;

  // brightness is set to maximum from bootloader if the persistent brightness sticky register is
  // not set.
  double brightness_ = 1.0;
  uint16_t scale_ = {};
  uint16_t calibrated_scale_ = {};
  bool power_ = true;
  uint8_t cfg2_;
  std::optional<double> max_absolute_brightness_nits_;

  inspect::DoubleProperty brightness_property_;
  inspect::UintProperty persistent_brightness_property_;
  inspect::UintProperty scale_property_;
  inspect::UintProperty calibrated_scale_property_;
  inspect::BoolProperty power_property_;
  inspect::DoubleProperty max_absolute_brightness_nits_property_;
  inspect::DoubleProperty power_watts_property_;
  inspect::UintProperty board_pid_property_;
  inspect::UintProperty panel_id_property_;
  inspect::UintProperty panel_type_property_;
  std::vector<fuchsia_hardware_ti_metadata::Register> registers_;
  inspect::Inspector inspector_;

  display::PanelType panel_type_ = display::PanelType::kUnknown;
  uint32_t board_pid_ = 0;
  double backlight_power_ = 0;
  double max_current_ = 0.0;

  fidl::ServerBindingGroup<fuchsia_hardware_backlight::Device> backlight_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_power_sensor::Device> power_sensor_bindings_;
};

}  // namespace ti

#endif  // SRC_UI_BACKLIGHT_DRIVERS_TI_LP8556_TI_LP8556_H_
