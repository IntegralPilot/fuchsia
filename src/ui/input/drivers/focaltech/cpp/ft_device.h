// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_FOCALTECH_CPP_FT_DEVICE_H_
#define SRC_UI_INPUT_DRIVERS_FOCALTECH_CPP_FT_DEVICE_H_

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.input.focaltech/cpp/fidl.h>
#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/stdcompat/inplace_vector.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/mutex.h>
#include <hwreg/bitfields.h>

#include "src/devices/i2c/lib/i2c-channel/i2c-channel.h"
#include "src/ui/input/lib/input-report-reader/reader.h"

// clang-format off
#define FTS_REG_CURPOINT                    0x02
#define FTS_REG_FINGER_START                0x03
#define FTS_REG_INT_CNT                     0x8F
#define FTS_REG_FLOW_WORK_CNT               0x91
#define FTS_REG_WORKMODE                    0x00
#define FTS_REG_WORKMODE_FACTORY_VALUE      0x40
#define FTS_REG_WORKMODE_WORK_VALUE         0x00
#define FTS_REG_ESDCHECK_DISABLE            0x8D
#define FTS_REG_CHIP_ID                     0xA3
#define FTS_REG_CHIP_ID2                    0x9F
#define FTS_REG_POWER_MODE                  0xA5
#define FTS_REG_POWER_MODE_SLEEP_VALUE      0x03
#define FTS_REG_FW_VER                      0xA6
#define FTS_REG_VENDOR_ID                   0xA8
#define FTS_REG_LCD_BUSY_NUM                0xAB
#define FTS_REG_FACE_DEC_MODE_EN            0xB0
#define FTS_REG_FACE_DEC_MODE_STATUS        0x01
#define FTS_REG_IDE_PARA_VER_ID             0xB5
#define FTS_REG_IDE_PARA_STATUS             0xB6
#define FTS_REG_GLOVE_MODE_EN               0xC0
#define FTS_REG_COVER_MODE_EN               0xC1
#define FTS_REG_CHARGER_MODE_EN             0x8B
#define FTS_REG_GESTURE_EN                  0xD0
#define FTS_REG_GESTURE_OUTPUT_ADDRESS      0xD3
#define FTS_REG_MODULE_ID                   0xE3
#define FTS_REG_LIC_VER                     0xE4
#define FTS_REG_ESD_SATURATE                0xED
#define FTS_REG_TYPE                        0xA0  // Chip model number (refer to datasheet)
#define FTS_REG_FIRMID                      0xA6  // Firmware version
#define FTS_REG_VENDOR_ID                   0xA8
#define FTS_REG_PANEL_ID                    0xAC
#define FTS_REG_RELEASE_ID_HIGH             0xAE  // Firmware release ID (two bytes)
#define FTS_REG_RELEASE_ID_LOW              0xAF
#define FTS_REG_IC_VERSION                  0xB1
// clang-format on

namespace ft {

enum class FtTouchEventType : uint8_t {
  kDown = 0,
  kUp = 1,
  kContact = 2,
};

// FocalTech FT3x27 CTPM Application Note Rev 0.1, Section 3.1 "Working Mode",
// pages 9-11.
struct TouchRecord {
  // Application note name: TOUCHx_XH
  uint8_t x_high;

  // Application note name: TOUCHx_XL
  uint8_t x_low;

  // Application note name: TOUCHx_YH
  uint8_t y_high;

  // Application note name: TOUCHx_YL
  uint8_t y_low;

  // Application note name: TOUCHx_WEIGHT
  uint8_t weight;

  // Application note name: TOUCHx_MISC
  uint8_t misc;

  DEF_ENUM_SUBFIELD(x_high, FtTouchEventType, 7, 6, event_type);
  DEF_SUBFIELD(x_high, 3, 0, x_position11_8);
  DEF_SUBFIELD(x_low, 7, 0, x_position7_0);
  DEF_SUBFIELD(y_high, 7, 4, touch_id);
  DEF_SUBFIELD(y_high, 3, 0, y_position11_8);
  DEF_SUBFIELD(y_low, 7, 0, y_position7_0);
  DEF_SUBFIELD(weight, 7, 0, touch_pressure);
  DEF_SUBFIELD(misc, 7, 4, touch_area);

  uint16_t x() const { return static_cast<uint16_t>((x_position11_8() << 8) | x_position7_0()); }
  uint16_t y() const { return static_cast<uint16_t>((y_position11_8() << 8) | y_position7_0()); }
  uint8_t finger_id() const { return touch_id(); }
};
static_assert(sizeof(TouchRecord) == 6);

class FtDevice : public fdf::DriverBase2,
                 public fidl::WireServer<fuchsia_input_report::InputDevice> {
 public:
  static constexpr std::string_view kDriverName = "focaltech_touch";
  static constexpr std::string_view kChildNodeName = "focaltouch-HidDevice";

  explicit FtDevice();

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  // Max unacknowledged report count allowed for 1/2 second.
  // FocalTech touchscreens are interrupt-driven.
  // Assuming a standard max event rate of 120 Hz yields 60 reports per 1/2 second (120 Hz / 2).
  static constexpr uint16_t kHalfSecondReportCount = 60;

  // fidl::WireServer<fuchsia_input_report::InputDevice> implementation.
  void GetInputReportsReader(GetInputReportsReaderRequestView request,
                             GetInputReportsReaderCompleter::Sync& completer) override;
  void GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                               GetInputReportsReaderV2Completer::Sync& completer) override;
  void GetDescriptor(GetDescriptorCompleter::Sync& completer) override;
  void SendOutputReport(SendOutputReportRequestView request,
                        SendOutputReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetFeatureReport(GetFeatureReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void SetFeatureReport(SetFeatureReportRequestView request,
                        SetFeatureReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetInputReport(GetInputReportRequestView request,
                      GetInputReportCompleter::Sync& completer) override {
    ZX_DEBUG_ASSERT(false);
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputDevice> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::warn("Unexpected fidl method invoked: {}", metadata.method_ordinal);
  }

 private:
  static constexpr size_t kFeatureAndDescriptorBufferSize = 512;

  /* Note: the focaltouch device is connected via i2c and is NOT a HID
      device.  This driver reads a collection of data from the data and
      parses it into a message which will be sent up the stack.  This message
      complies with a HID descriptor that manually scripted (i.e. - not
      reported by the device iteself).
  */
  // Number of touch points this device can report simultaneously
  static constexpr uint32_t kMaxPoints = 10;
  // Size of each individual touch record (note: there are kMaxPoints of
  //  them) on the i2c bus.  This is not the HID report size.
  static constexpr uint32_t kTouchRecordSize = 6;

  static constexpr size_t kMaxI2cTransferLength = 8;

  struct FtInputReport {
    zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);
    struct Contact {
      uint8_t finger_id = 0;
      uint16_t x = 0;
      uint16_t y = 0;
    };
    cpp26::inplace_vector<Contact, kMaxPoints> contacts;

    void ToFidlInputReport(
        fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
        fidl::AnyArena& allocator) const;
  };

  static uint8_t CalculateEcc(std::span<const uint8_t> buffer, uint8_t initial = 0);

  // Enters romboot and returns true if firmware download is needed, returns false otherwise.
  zx::result<bool> CheckFirmwareAndStartRomboot(uint8_t firmware_version);
  zx::result<> StartRomboot();
  zx_status_t WaitForRomboot();

  zx::result<uint16_t> GetBootId();

  // Returns true if the expected value was read before the timeout, false if not.
  zx::result<bool> WaitForFlashStatus(uint16_t expected_value, int tries, zx::duration retry_sleep);

  zx::result<> EraseFlash(size_t firmware_size);
  zx::result<> SendFirmware(cpp20::span<const uint8_t> firmware);
  zx::result<> SendFirmwarePacket(uint32_t address, std::span<const uint8_t> packet);
  zx::result<> CheckFirmwareEcc(size_t size, uint8_t expected_ecc);

  zx::result<uint8_t> ReadReg8(uint8_t address);
  zx::result<uint16_t> ReadReg16(uint8_t address);

  zx::result<> Write8(uint8_t value);
  zx::result<> WriteReg8(uint8_t address, uint8_t value);
  zx::result<> WriteReg16(uint8_t address, uint16_t value);

  zx::result<uint8_t> Read(uint8_t addr);
  zx::result<> Read(uint8_t addr, std::span<uint8_t> dst);

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);

  static FtInputReport ParseReport(std::span<const uint8_t> buf);

  void LogRegisterValue(uint8_t addr, std::string_view name);

  zx_status_t UpdateFirmwareIfNeeded(const fuchsia_hardware_input_focaltech::Metadata& metadata,
                                     display::PanelType panel_type);

  void DevfsConnect(fidl::ServerEnd<fuchsia_input_report::InputDevice> server);

  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> int_gpio_;
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> reset_gpio_;
  zx::interrupt irq_;
  async::IrqMethod<FtDevice, &FtDevice::HandleIrq> irq_handler_{this};
  i2c::I2cChannel i2c_;

  inspect::Inspector inspector_;
  inspect::Node node_;
  inspect::ValueList values_;

  inspect::Node metrics_root_;
  inspect::UintProperty average_latency_usecs_;
  inspect::UintProperty max_latency_usecs_;
  inspect::UintProperty total_report_count_;
  inspect::UintProperty last_event_timestamp_;

  uint64_t report_count_ = 0;
  zx::duration total_latency_;
  zx::duration max_latency_;

  input_report_reader::InputReportReaderManager<FtInputReport> readers_;
  uint32_t x_max_;
  uint32_t y_max_;

  fidl::ServerBindingGroup<fuchsia_input_report::InputDevice> bindings_;
  driver_devfs::Connector<fuchsia_input_report::InputDevice> devfs_connector_{
      fit::bind_member<&FtDevice::DevfsConnect>(this)};
  fdf::OwnedChildNode child_;
};
}  // namespace ft

#endif  // SRC_UI_INPUT_DRIVERS_FOCALTECH_CPP_FT_DEVICE_H_
