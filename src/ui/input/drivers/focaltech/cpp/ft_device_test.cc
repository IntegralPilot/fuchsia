// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ft_device.h"

#include <lib/ddk/metadata.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-i2c/fake-i2c.h>
#include <lib/zx/clock.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstddef>

#include <gtest/gtest.h>

#include "ft_firmware.h"
#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/lib/testing/predicates/status.h"

namespace {

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
// Firmware must be at least 0x120 bytes. Add some extra size to make them different.
constexpr uint8_t kFirmware0[0x120 + 0] = {0x00, 0xd2, 0xc8, 0x53, [0x10a] = 0xd5};
constexpr uint8_t kFirmware1[0x120 + 1] = {0x10, 0x58, 0xb2, 0x12, [0x10a] = 0xc8};
constexpr uint8_t kFirmware2[0x120 + 2] = {0xb7, 0xf9, 0xd1, 0x12, [0x10a] = 0xb0};
constexpr uint8_t kFirmware3[0x120 + 3] = {0x02, 0x69, 0x96, 0x71, [0x10a] = 0x61};
#pragma GCC diagnostic pop

}  // namespace

namespace ft {

namespace fi2c = fuchsia_hardware_i2c;
namespace fgpio = fuchsia_hardware_gpio;

const FirmwareEntry kFirmwareEntries[] = {
    {
        .panel_type = display::PanelType::kInnoluxP101dezFitipowerJd9364,
        .firmware_data = kFirmware0,
        .firmware_size = sizeof(kFirmware0),
    },
    {
        .panel_type = display::PanelType::kBoeTv101wxmFitipowerJd9364,
        .firmware_data = kFirmware1,
        .firmware_size = sizeof(kFirmware1),
    },
    {
        .panel_type = display::PanelType::kBoeTv101wxmFitipowerJd9365,
        .firmware_data = kFirmware2,
        .firmware_size = sizeof(kFirmware2),
    },
    {
        .panel_type = display::PanelType::kBoeTv070wsmFitipowerJd9364Astro,
        .firmware_data = kFirmware3,
        .firmware_size = sizeof(kFirmware3),
    },
};

const size_t kNumFirmwareEntries = std::size(kFirmwareEntries);

class FakeFtDevice : public fake_i2c::FakeI2c {
 public:
  ~FakeFtDevice() { ZX_ASSERT(expected_report_.empty()); }

  uint32_t firmware_write_size() const { return firmware_write_size_; }

  void ExpectReport(uint8_t addr, std::vector<uint8_t> report) {
    expected_report_.emplace(addr, std::move(report));
  }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    if (write_buffer_size < 1) {
      return ZX_ERR_IO;
    }

    *read_buffer_size = 0;

    if (write_buffer[0] == 0xa3) {  // Chip core register
      read_buffer[0] = 0x58;        // Firmware is valid
      *read_buffer_size = 1;
    } else if (write_buffer[0] == 0xa6) {  // Chip firmware version
      read_buffer[0] = kFirmware1[0x10a];  // Set to a known version to test the up-to-date case
      *read_buffer_size = 1;
    } else if (write_buffer[0] == 0xfc && write_buffer_size == 2) {  // Chip work mode
      if (write_buffer[1] != 0xaa && write_buffer[1] != 0x55) {      // Soft reset
        return ZX_ERR_IO;
      }
    } else if (write_buffer[0] == 0xeb && write_buffer_size == 3) {  // HID to STD
      if (write_buffer[1] != 0xaa || write_buffer[2] != 0x09) {
        return ZX_ERR_IO;
      }
    } else if (write_buffer[0] == 0x55 && write_buffer_size == 1) {  // Unlock boot
    } else if (write_buffer[0] == 0x90 && write_buffer_size == 1) {  // Boot ID
      read_buffer[0] = 0x58;
      read_buffer[1] = 0x2c;
      *read_buffer_size = 2;
    } else if (write_buffer[0] == 0x09 && write_buffer_size == 2) {  // Flash erase
      if (write_buffer[1] != 0x0b) {                                 // Erase app area
        return ZX_ERR_IO;
      }
    } else if (write_buffer[0] == 0xb0 && write_buffer_size == 4) {  // Set erase size
    } else if (write_buffer[0] == 0x61 && write_buffer_size == 1) {  // Start erase
      ecc_ = 0;
      flash_status_ = 0xf0aa;
    } else if (write_buffer[0] == 0x6a && write_buffer_size == 1) {  // Read flash status
      read_buffer[0] = flash_status_ >> 8;
      read_buffer[1] = flash_status_ & 0xff;
      *read_buffer_size = 2;
    } else if (write_buffer[0] == 0xbf && write_buffer_size >= 6) {  // Firmware packet
      const uint32_t address = (write_buffer[1] << 16) | (write_buffer[2] << 8) | write_buffer[3];
      const auto packet_size = static_cast<uint8_t>((write_buffer[4] << 8) | write_buffer[5]);

      if ((packet_size + 6) != write_buffer_size) {
        return ZX_ERR_IO;
      }

      for (uint32_t i = 6; i < write_buffer_size; i++) {
        ecc_ ^= write_buffer[i];
      }

      flash_status_ = (0x1000 + (address / packet_size)) & 0xffff;
      firmware_write_size_ += packet_size;  // Ignore overlapping addresses.
    } else if (write_buffer[0] == 0x64 && write_buffer_size == 1) {  // ECC initialization
    } else if (write_buffer[0] == 0x65 && write_buffer_size == 6) {  // Start ECC calculation
      flash_status_ = 0xf055;                                        // ECC calculation done
    } else if (write_buffer[0] == 0x66 && write_buffer_size == 1) {  // Read calculated ECC
      read_buffer[0] = ecc_;
      *read_buffer_size = 1;
    } else if (write_buffer[0] == 0x07 && write_buffer_size == 1) {  // Reset
    } else if ((write_buffer[0] == FTS_REG_TYPE || write_buffer[0] == FTS_REG_FIRMID ||
                write_buffer[0] == FTS_REG_VENDOR_ID || write_buffer[0] == FTS_REG_PANEL_ID ||
                write_buffer[0] == FTS_REG_RELEASE_ID_HIGH ||
                write_buffer[0] == FTS_REG_RELEASE_ID_LOW ||
                write_buffer[0] == FTS_REG_IC_VERSION) &&
               write_buffer_size == 1) {  // LogRegisterValue
      read_buffer[0] = 0;
      *read_buffer_size = 1;
    } else if (write_buffer_size == 1) {  // Read report
      EXPECT_FALSE(expected_report_.empty());
      EXPECT_EQ(write_buffer[0], expected_report_.front().first);
      memcpy(read_buffer, expected_report_.front().second.data(),
             expected_report_.front().second.size());
      *read_buffer_size = expected_report_.front().second.size();
      expected_report_.pop();
    }

    return ZX_OK;
  }

 private:
  uint16_t flash_status_ = 0;
  uint8_t ecc_ = 0;
  uint32_t firmware_write_size_ = 0;

  std::queue<std::pair<uint8_t, std::vector<uint8_t>>> expected_report_;  // address, data pair
};

class FtDeviceTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(zx::interrupt interrupt, const fuchsia_hardware_input_focaltech::Metadata& metadata,
            const std::optional<display::PanelType>& panel_type) {
    interrupt_gpio_.SetInterrupt(zx::ok(std::move(interrupt)));

    pdev_.AddFidlMetadata(fuchsia_hardware_input_focaltech::Metadata::kSerializableName, metadata);

    device_server_.Initialize("pdev", std::nullopt, {});
    if (panel_type.has_value()) {
      device_server_.AddMetadata(DEVICE_METADATA_DISPLAY_PANEL_TYPE, &panel_type.value(),
                                 sizeof(panel_type.value()));
    }
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    EXPECT_OK(device_server_.Serve(dispatcher, &to_driver_vfs));
    EXPECT_OK(
        to_driver_vfs.AddService<fi2c::Service>(i2c_.CreateInstanceHandler(dispatcher), "i2c"));
    EXPECT_OK(to_driver_vfs.AddService<fgpio::Service>(reset_gpio_.CreateInstanceHandler(),
                                                       "gpio-reset"));
    EXPECT_OK(to_driver_vfs.AddService<fgpio::Service>(interrupt_gpio_.CreateInstanceHandler(),
                                                       "gpio-int"));
    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(dispatcher), "pdev"));

    return zx::ok();
  }

  FakeFtDevice& i2c() { return i2c_; }
  fake_gpio::FakeGpio& interrupt_gpio() { return interrupt_gpio_; }
  fake_gpio::FakeGpio& reset_gpio() { return reset_gpio_; }

 private:
  FakeFtDevice i2c_;
  fake_gpio::FakeGpio interrupt_gpio_;
  fake_gpio::FakeGpio reset_gpio_;
  compat::DeviceServer device_server_;
  fdf_fake::FakePDev pdev_;
};

class FixtureConfig final {
 public:
  using DriverType = FtDevice;
  using EnvironmentType = FtDeviceTestEnvironment;
};

class FocaltechTest : public testing::Test {
 public:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void StartDriver(const fuchsia_hardware_input_focaltech::Metadata& metadata,
                   const std::optional<display::PanelType>& panel_type = std::nullopt) {
    zx::interrupt interrupt;
    ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
    ASSERT_EQ(ZX_OK, interrupt.duplicate(ZX_RIGHT_SAME_RIGHTS, &interrupt_));

    driver_test_.RunInEnvironmentTypeContext([interrupt = std::move(interrupt), &metadata,
                                              &panel_type](FtDeviceTestEnvironment& env) mutable {
      env.Init(std::move(interrupt), metadata, panel_type);
    });
    ASSERT_OK(driver_test_.StartDriver());
    VerifyGpioInit();

    zx::result input_device = driver_test_.ConnectThroughDevfs<fuchsia_input_report::InputDevice>(
        FtDevice::kChildNodeName);
    ASSERT_OK(input_device);
    input_device_.Bind(std::move(input_device.value()),
                       driver_test_.runtime().GetForegroundDispatcher()->async_dispatcher());
  }

  fidl::WireClient<fuchsia_input_report::InputDevice>& input_device() { return input_device_; }
  zx::interrupt& interrupt() { return interrupt_; }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  void VerifyGpioInit() {
    driver_test_.RunInEnvironmentTypeContext([](FtDeviceTestEnvironment& env) {
      std::vector interrupt_states = env.interrupt_gpio().GetStateLog();
      ASSERT_GE(interrupt_states.size(), size_t(1));
      ASSERT_EQ(fake_gpio::ReadSubState{}, interrupt_states[0].sub_state);

      std::vector reset_states = env.reset_gpio().GetStateLog();
      ASSERT_GE(reset_states.size(), size_t(2));
      ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, reset_states[0].sub_state);
      ASSERT_EQ(fake_gpio::WriteSubState{.value = 1}, reset_states[1].sub_state);
    });
  }

  zx::interrupt interrupt_;
  fidl::WireClient<fuchsia_input_report::InputDevice> input_device_;

  // Use `ForegroundDriverTest` instead of `BackgroundDriverTest` because some tests send one-way
  // FIDL requests to the driver and wait for the request to be handled by the driver. This can be
  // done with `runtime().RunUntilIdle()` if the driver is running on the foreground dispatcher.
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

void VerifyDescriptor(const fuchsia_input_report::wire::DeviceDescriptor& descriptor, int64_t x_max,
                      int64_t y_max) {
  EXPECT_TRUE(descriptor.has_device_information());
  EXPECT_EQ(descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(descriptor.device_information().product_id(),
            static_cast<uint32_t>(
                fuchsia_input_report::wire::VendorGoogleProductId::kFocaltechTouchscreen));

  EXPECT_TRUE(descriptor.has_touch());
  EXPECT_FALSE(descriptor.has_consumer_control());
  EXPECT_FALSE(descriptor.has_keyboard());
  EXPECT_FALSE(descriptor.has_mouse());
  EXPECT_FALSE(descriptor.has_sensor());

  EXPECT_TRUE(descriptor.touch().has_input());
  EXPECT_FALSE(descriptor.touch().has_feature());

  EXPECT_TRUE(descriptor.touch().input().has_touch_type());
  EXPECT_EQ(descriptor.touch().input().touch_type(),
            fuchsia_input_report::wire::TouchType::kTouchscreen);

  EXPECT_TRUE(descriptor.touch().input().has_max_contacts());
  EXPECT_EQ(descriptor.touch().input().max_contacts(), uint32_t(10));

  EXPECT_FALSE(descriptor.touch().input().has_buttons());
  EXPECT_TRUE(descriptor.touch().input().has_contacts());
  EXPECT_EQ(descriptor.touch().input().contacts().size(), size_t(10));

  for (const auto& c : descriptor.touch().input().contacts()) {
    EXPECT_TRUE(c.has_position_x());
    EXPECT_TRUE(c.has_position_y());
    EXPECT_FALSE(c.has_contact_height());
    EXPECT_FALSE(c.has_contact_width());
    EXPECT_FALSE(c.has_pressure());

    EXPECT_EQ(c.position_x().range.min, 0);
    EXPECT_EQ(c.position_x().range.max, x_max);
    EXPECT_EQ(c.position_x().unit.type, fuchsia_input_report::wire::UnitType::kOther);
    EXPECT_EQ(c.position_x().unit.exponent, 0);

    EXPECT_EQ(c.position_y().range.min, 0);
    EXPECT_EQ(c.position_y().range.max, y_max);
    EXPECT_EQ(c.position_y().unit.type, fuchsia_input_report::wire::UnitType::kOther);
    EXPECT_EQ(c.position_y().unit.exponent, 0);
  }
}

TEST_F(FocaltechTest, Metadata3x27) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt3x27Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt3X27,
      .needs_firmware = false,
  });
  StartDriver(kFt3x27Metadata);

  input_device()->GetDescriptor().ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetDescriptor>& result) {
        ASSERT_OK(result.status());
        VerifyDescriptor(result->descriptor, 600, 1024);
      });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(FocaltechTest, Metadata5726) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt5726Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt5726,
      .needs_firmware = false,
  });
  StartDriver(kFt5726Metadata);

  input_device()->GetDescriptor().ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetDescriptor>& result) {
        EXPECT_TRUE(result.ok());
        VerifyDescriptor(result->descriptor, 800, 1280);
      });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(FocaltechTest, Metadata6336) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt6336Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt6336,
      .needs_firmware = false,
  });
  StartDriver(kFt6336Metadata);

  input_device()->GetDescriptor().ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetDescriptor>& result) {
        EXPECT_TRUE(result.ok());
        VerifyDescriptor(result->descriptor, 480, 800);
      });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(FocaltechTest, Firmware5726) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt5726Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt5726,
      .needs_firmware = true,
  });
  static constexpr display::PanelType kPanelType = display::PanelType::kBoeTv101wxmFitipowerJd9365;
  StartDriver(kFt5726Metadata, kPanelType);

  driver_test().RunInEnvironmentTypeContext([](FtDeviceTestEnvironment& env) {
    EXPECT_EQ(env.i2c().firmware_write_size(), sizeof(kFirmware2));
  });
}

TEST_F(FocaltechTest, Firmware5726UpToDate) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt5726Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt5726,
      .needs_firmware = true,
  });
  constexpr display::PanelType kPanelType = display::PanelType::kBoeTv101wxmFitipowerJd9364;
  StartDriver(kFt5726Metadata, kPanelType);

  driver_test().RunInEnvironmentTypeContext(
      [](FtDeviceTestEnvironment& env) { EXPECT_EQ(env.i2c().firmware_write_size(), 0u); });
}

class ReaderV2EventHandler
    : public fidl::WireAsyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
 public:
  explicit ReaderV2EventHandler(
      fit::function<
          void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
          callback)
      : callback_(std::move(callback)) {}

  void OnInputReports(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) override {
    if (callback_) {
      callback_(event);
    }
  }

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

 private:
  fit::function<void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
      callback_;
};

TEST_F(FocaltechTest, Touch) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt6336Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt6336,
      .needs_firmware = false,
  });
  StartDriver(kFt6336Metadata);

  fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> reader_endpoints =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  input_device()
      ->GetInputReportsReaderV2(std::move(reader_endpoints.server), 10)
      .ThenExactlyOnce(
          [](fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2>&
                 result) {
            zx_status_t status = result.status();
            ASSERT_OK(status);
          });

  bool got_report = false;
  std::unique_ptr<ReaderV2EventHandler> event_handler = std::make_unique<ReaderV2EventHandler>(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(size_t(1), reports.size());
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), size_t(2));
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), uint32_t(0));
        EXPECT_EQ(touch_report.contacts()[0].position_x(), 0x001);
        EXPECT_EQ(touch_report.contacts()[0].position_y(), 0x013);

        EXPECT_EQ(touch_report.contacts()[1].contact_id(), uint32_t(1));
        EXPECT_EQ(touch_report.contacts()[1].position_x(), 0x031);
        EXPECT_EQ(touch_report.contacts()[1].position_y(), 0x000);
        got_report = true;
      });

  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(reader_endpoints.client),
      driver_test().runtime().GetForegroundDispatcher()->async_dispatcher(), event_handler.get());

  // Wait for the driver to receive the `GetInputReportsReaderV2()` request and create an
  // input-reports reader.
  driver_test().runtime().RunUntilIdle();

  // clang-format off
  static const std::array<uint8_t, 61> kExpectedReport = {
      0x02,  // contact_count

      // Contact 0, finger_id = 0
      0x80, 0x01,  // x = 0x001
      0x00, 0x13,  // y = 0x013
      0x00, 0x00,

      // Contact 1, finger_id = 1
      0x80, 0x31,  // x = 0x031
      0x10, 0x00,  // y = 0x000
      0x00, 0x00,

      // Contact 2
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 3
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 4
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 5
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 6
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 7
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 8
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

      // Contact 9
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  };
  // clang-format on
  driver_test().RunInEnvironmentTypeContext([](FtDeviceTestEnvironment& env) {
    for (size_t i = 0; i < sizeof(kExpectedReport); i += 8) {
      env.i2c().ExpectReport(
          static_cast<uint8_t>(i + FTS_REG_CURPOINT),
          std::vector<uint8_t>(kExpectedReport.begin() + i,
                               kExpectedReport.begin() + std::min(i + 8, sizeof(kExpectedReport))));
    }
  });
  interrupt().trigger(0, zx::clock::get_boot());

  driver_test().runtime().RunUntilIdle();
  EXPECT_TRUE(got_report);
}

TEST_F(FocaltechTest, TouchWithGap) {
  static const fuchsia_hardware_input_focaltech::Metadata kFt6336Metadata({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt6336,
      .needs_firmware = false,
  });
  StartDriver(kFt6336Metadata);

  fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> reader_endpoints =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  input_device()
      ->GetInputReportsReaderV2(std::move(reader_endpoints.server), 10)
      .ThenExactlyOnce(
          [](fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2>&
                 result) {
            zx_status_t status = result.status();
            ASSERT_OK(status);
          });

  bool got_report = false;
  std::unique_ptr<ReaderV2EventHandler> event_handler = std::make_unique<ReaderV2EventHandler>(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(size_t(1), reports.size());
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), size_t(1));
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), uint32_t(1));
        EXPECT_EQ(touch_report.contacts()[0].position_x(), 0x031);
        EXPECT_EQ(touch_report.contacts()[0].position_y(), 0x000);
        got_report = true;
      });

  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(reader_endpoints.client),
      driver_test().runtime().GetForegroundDispatcher()->async_dispatcher(), event_handler.get());

  driver_test().runtime().RunUntilIdle();

  // clang-format off
  static const std::array<uint8_t, 61> kExpectedReport = {
      0x02,  // contact_count reported by hardware (slots 0 and 1)

      // Contact 0 (UP event = 0x40), should be ignored
      0x40, 0x01,  // x = 0x001
      0x00, 0x13,  // y = 0x013
      0x00, 0x00,

      // Contact 1 (CONTACT event = 0x80), finger_id = 1
      0x80, 0x31,  // x = 0x031
      0x10, 0x00,  // y = 0x000
      0x00, 0x00,

      // Remaining slots empty
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  };
  // clang-format on
  driver_test().RunInEnvironmentTypeContext([](FtDeviceTestEnvironment& env) {
    for (size_t i = 0; i < sizeof(kExpectedReport); i += 8) {
      env.i2c().ExpectReport(
          static_cast<uint8_t>(i + FTS_REG_CURPOINT),
          std::vector<uint8_t>(kExpectedReport.begin() + i,
                               kExpectedReport.begin() + std::min(i + 8, sizeof(kExpectedReport))));
    }
  });
  interrupt().trigger(0, zx::clock::get_boot());

  driver_test().runtime().RunUntilIdle();
  EXPECT_TRUE(got_report);
}

TEST(TouchRecordTest, XYDecode) {
  TouchRecord record;

  record.x_high = 0x01;  // event_flag 0 (DOWN), x_high 1
  record.x_low = 0x23;   // x_low 0x23
  record.y_high = 0x74;  // touch_id 7, y_high 4
  record.y_low = 0x56;   // y_low 0x56

  // x = 0x123 (high bits [11:8] 0x01, low bits [7:0] 0x23)
  // y = 0x456 (high bits [11:8] 0x04, low bits [7:0] 0x56)
  // finger_id = 7

  EXPECT_EQ(record.x(), 0x123);
  EXPECT_EQ(record.y(), 0x456);
  EXPECT_EQ(record.finger_id(), 7);
  EXPECT_EQ(record.event_type(), FtTouchEventType::kDown);
}

}  // namespace ft
