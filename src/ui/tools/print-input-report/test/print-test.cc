// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/ddk/device.h>
#include <lib/hid/usages.h>
#include <lib/sync/completion.h>

#include <string>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <gtest/gtest.h>

#include "fuchsia/input/report/cpp/fidl.h"
#include "src/ui/input/testing/fake_input_report_device/fake.h"
#include "src/ui/tools/print-input-report/devices.h"
#include "src/ui/tools/print-input-report/printer.h"

namespace test {

class FakePrinter : public print_input_report::Printer {
 public:
  void RealPrint(const char* format, va_list argptr) override {
    char buf[kMaxBufLen];
    vsprintf(buf, format, argptr);

    ASSERT_LT(current_string_index_, expected_strings_.size());
    const std::string& expected = expected_strings_[current_string_index_];
    current_string_index_++;

    // Check that we match the expected string.
    ASSERT_GT(expected.size(), indent_);
    int cmp = strcmp(buf, expected.c_str());
    if (cmp != 0) {
      printf("Wanted string: '%s'\n", expected.c_str());
      printf("Saw string:    '%s'\n", buf);
      ASSERT_TRUE(false);
    }

    // Print the string for easy debugging.
    vprintf(format, argptr);

    va_end(argptr);
  }

  void SetExpectedStrings(const std::vector<std::string>& strings) {
    current_string_index_ = 0;
    expected_strings_ = strings;
  }

  void AssertSawAllStrings() { ASSERT_EQ(current_string_index_, expected_strings_.size()); }

 private:
  static constexpr size_t kMaxBufLen = 1024;
  size_t current_string_index_ = 0;
  std::vector<std::string> expected_strings_;
};

class PrintInputReport : public testing::Test {
 protected:
  virtual void SetUp() {
    // Make the channels and the fake device.
    zx::channel token_server, token_client;
    ASSERT_EQ(zx::channel::create(0, &token_server, &token_client), ZX_OK);

    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);

    fake_device_ = std::make_unique<fake_input_report_device::FakeInputDevice>(
        fidl::InterfaceRequest<fuchsia::input::report::InputDevice>(std::move(token_server)),
        loop_->dispatcher());

    client_.emplace(fidl::ClientEnd<fuchsia_input_report::InputDevice>(std::move(token_client)),
                    loop_->dispatcher());
  }

  virtual void TearDown() { loop_->Quit(); }

  std::unique_ptr<fake_input_report_device::FakeInputDevice> fake_device_;
  std::unique_ptr<async::Loop> loop_;
  std::optional<fidl::WireSharedClient<fuchsia_input_report::InputDevice>> client_;
};

TEST_F(PrintInputReport, PrintMouseInputReport) {
  fuchsia::input::report::InputReport report;
  report.mutable_mouse()->set_movement_x(100);
  report.mutable_mouse()->set_movement_y(200);
  report.mutable_mouse()->set_position_x(300);
  report.mutable_mouse()->set_position_y(400);
  report.mutable_mouse()->set_scroll_v(100);
  report.mutable_mouse()->set_pressed_buttons({1, 10, 5});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Movement x: 00000100\n",
      "Movement y: 00000200\n",
      "Position x: 00000300\n",
      "Position y: 00000400\n",
      "Scroll v: 00000100\n",
      "Button 01 pressed\n",
      "Button 10 pressed\n",
      "Button 05 pressed\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintMouseInputDescriptor) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  auto mouse = descriptor->mutable_mouse()->mutable_input();

  fuchsia::input::report::Axis axis;
  axis.unit.type = fuchsia::input::report::UnitType::METERS;
  axis.unit.exponent = 0;
  axis.range.min = -100;
  axis.range.max = -100;
  mouse->set_movement_x(axis);

  axis.unit.type = fuchsia::input::report::UnitType::NONE;
  axis.range.min = -200;
  axis.range.max = -200;
  mouse->set_movement_y(axis);

  axis.range.min = 300;
  axis.range.max = 300;
  mouse->set_position_x(axis);

  axis.range.min = 400;
  axis.range.max = 400;
  mouse->set_position_y(axis);

  mouse->set_buttons({1, 10, 5});

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Mouse Descriptor:\n",
      "  Movement X:\n",
      "    Unit:   METERS\n",
      "    Min:      -100\n",
      "    Max:      -100\n",
      "  Movement Y:\n",
      "    Unit:     NONE\n",
      "    Min:      -200\n",
      "    Max:      -200\n",
      "  Position X:\n",
      "    Unit:     NONE\n",
      "    Min:       300\n",
      "    Max:       300\n",
      "  Position Y:\n",
      "    Unit:     NONE\n",
      "    Min:       400\n",
      "    Max:       400\n",
      "  Button: 1\n",
      "  Button: 10\n",
      "  Button: 5\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));

  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintSensorInputDescriptor) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  descriptor->mutable_sensor()->mutable_input()->emplace_back();
  auto values = descriptor->mutable_sensor()->mutable_input()->back().mutable_values();

  fuchsia::input::report::SensorAxis axis;
  axis.axis.unit.type = fuchsia::input::report::UnitType::SI_LINEAR_VELOCITY;
  axis.axis.unit.exponent = 0;
  axis.axis.range.min = 0;
  axis.axis.range.max = 1000;
  axis.type = fuchsia::input::report::SensorType::ACCELEROMETER_X;

  values->push_back(axis);

  axis.axis.unit.type = fuchsia::input::report::UnitType::LUX;
  axis.type = fuchsia::input::report::SensorType::LIGHT_ILLUMINANCE;

  values->push_back(axis);

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Sensor Descriptor:\n",
      "  Value 00:\n",
      "    SensorType: ACCELEROMETER_X\n",
      "    Unit: SI_LINEAR_VELOCITY\n",
      "    Min:         0\n",
      "    Max:      1000\n",
      "  Value 01:\n",
      "    SensorType: LIGHT_ILLUMINANCE\n",
      "    Unit:      LUX\n",
      "    Min:         0\n",
      "    Max:      1000\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));
  loop_->RunUntilIdle();
}

TEST_F(PrintInputReport, PrintSensorInputReport) {
  fuchsia::input::report::InputReport report;
  report.mutable_sensor()->set_values({100, -100});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Sensor[00]: 00000100\n",
      "Sensor[01]: -0000100\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintTouchInputDescriptor) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  auto touch = descriptor->mutable_touch()->mutable_input();
  touch->set_touch_type(fuchsia::input::report::TouchType::TOUCHSCREEN);
  touch->set_max_contacts(100);

  fuchsia::input::report::Axis axis;
  axis.unit.type = fuchsia::input::report::UnitType::NONE;
  axis.unit.exponent = 0;
  axis.range.min = 0;
  axis.range.max = 300;

  fuchsia::input::report::ContactInputDescriptor contact;
  contact.set_position_x(axis);

  axis.range.max = 500;
  contact.set_position_y(axis);

  axis.range.max = 100;
  contact.set_pressure(axis);

  touch->mutable_contacts()->push_back(std::move(contact));

  descriptor->mutable_touch()->mutable_feature()->set_supports_input_mode(true);
  descriptor->mutable_touch()->mutable_feature()->set_supports_selective_reporting(true);

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Touch Descriptor:\n",
      "  Input Report:\n",
      "    Touch Type: TOUCHSCREEN\n",
      "    Max Contacts: 100\n",
      "    Contact: 00\n",
      "      Position X:\n",
      "        Unit:     NONE\n",
      "        Min:         0\n",
      "        Max:       300\n",
      "      Position Y:\n",
      "        Unit:     NONE\n",
      "        Min:         0\n",
      "        Max:       500\n",
      "      Pressure:\n",
      "        Unit:     NONE\n",
      "        Min:         0\n",
      "        Max:       100\n",
      "  Feature Report:\n",
      "    Supports InputMode: 1\n",
      "    Supports SelectiveReporting: 1\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));
  loop_->RunUntilIdle();
}

TEST_F(PrintInputReport, PrintTouchInputReport) {
  fuchsia::input::report::InputReport report;
  fuchsia::input::report::ContactInputReport contact;

  contact.set_contact_id(10);
  contact.set_position_x(123);
  contact.set_position_y(234);
  contact.set_pressure(345);
  contact.set_contact_width(678);
  contact.set_contact_height(789);

  report.mutable_touch()->mutable_contacts()->push_back(std::move(contact));

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Contact ID: 10\n",
      "  Position X:     00000123\n",
      "  Position Y:     00000234\n",
      "  Pressure:       00000345\n",
      "  Contact Width:  00000678\n",
      "  Contact Height: 00000789\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintTouchInputDescriptorWithButtons) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  auto touch = descriptor->mutable_touch()->mutable_input();
  touch->set_touch_type(fuchsia::input::report::TouchType::TOUCHSCREEN);
  touch->set_max_contacts(1);
  touch->set_buttons({fuchsia::input::report::TouchButton::PALM,
                      fuchsia::input::report::TouchButton::SWIPE_UP,
                      fuchsia::input::report::TouchButton::SWIPE_LEFT,
                      fuchsia::input::report::TouchButton::SWIPE_RIGHT,
                      fuchsia::input::report::TouchButton::SWIPE_DOWN});

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Touch Descriptor:\n",
      "  Input Report:\n",
      "    Touch Type: TOUCHSCREEN\n",
      "    Max Contacts: 1\n",
      "    Button: PALM\n",
      "    Button: SWIPE_UP\n",
      "    Button: SWIPE_LEFT\n",
      "    Button: SWIPE_RIGHT\n",
      "    Button: SWIPE_DOWN\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintTouchInputReportWithButtons) {
  fuchsia::input::report::InputReport report;
  report.mutable_touch()->set_pressed_buttons({fuchsia::input::report::TouchButton::SWIPE_UP,
                                               fuchsia::input::report::TouchButton::SWIPE_LEFT});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Button SWIPE_UP pressed\n",
      "Button SWIPE_LEFT pressed\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintTouchInputReportWithUnknownButtons) {
  fuchsia::input::report::InputReport report;
  report.mutable_touch()->set_pressed_buttons(
      {static_cast<fuchsia::input::report::TouchButton>(99)});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Button ERROR pressed\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintKeyboardDescriptor) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  descriptor->mutable_keyboard()->mutable_input()->set_keys3(
      {fuchsia::input::Key::A, fuchsia::input::Key::UP, fuchsia::input::Key::LEFT_SHIFT});
  descriptor->mutable_keyboard()->mutable_output()->set_leds(
      {fuchsia::input::report::LedType::CAPS_LOCK, fuchsia::input::report::LedType::SCROLL_LOCK});

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Keyboard Descriptor:\n",
      "Input Report:\n",
      "  Key:   458756\n",  // 0x70004
      "  Key:   458834\n",  // 0x70052
      "  Key:   458977\n",  // 0x700e1
      "Output Report:\n",
      "  Led: CAPS_LOCK\n",
      "  Led: SCROLL_LOCK\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));
  loop_->RunUntilIdle();
}

TEST_F(PrintInputReport, PrintKeyboardInputReport) {
  fuchsia::input::report::InputReport report;
  report.mutable_keyboard()->set_pressed_keys3(
      {fuchsia::input::Key::A, fuchsia::input::Key::UP, fuchsia::input::Key::LEFT_SHIFT});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Keyboard Report\n",
      "  Key:   458756\n",  // 0x70004
      "  Key:   458834\n",  // 0x70052
      "  Key:   458977\n",  // 0x700e1
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintKeyboardInputReportNoKeys) {
  fuchsia::input::report::InputReport report;
  report.mutable_keyboard()->set_pressed_keys3({});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "Keyboard Report\n",
      "  No keys pressed\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintConsumerControlDescriptor) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  descriptor->mutable_consumer_control()->mutable_input()->set_buttons(
      {fuchsia::input::report::ConsumerControlButton::VOLUME_UP,
       fuchsia::input::report::ConsumerControlButton::VOLUME_DOWN,
       fuchsia::input::report::ConsumerControlButton::FACTORY_RESET});

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "ConsumerControl Descriptor:\n",
      "Input Report:\n",
      "  Button:        VOLUME_UP\n",
      "  Button:      VOLUME_DOWN\n",
      "  Button:    FACTORY_RESET\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));

  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintConsumerControlReport) {
  fuchsia::input::report::InputReport report;
  report.mutable_consumer_control()->set_pressed_buttons(
      {fuchsia::input::report::ConsumerControlButton::VOLUME_UP,
       fuchsia::input::report::ConsumerControlButton::VOLUME_DOWN,
       fuchsia::input::report::ConsumerControlButton::FACTORY_RESET});

  std::vector<fuchsia::input::report::InputReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Report from file: test\n",
      "ConsumerControl Report\n",
      "  Button:        VOLUME_UP\n",
      "  Button:      VOLUME_DOWN\n",
      "  Button:    FACTORY_RESET\n",
      "\n",
  });

  print_input_report::PrintInputReports("test", &printer, client_->Clone(), loop_->dispatcher(), 1);
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintInputDescriptorWithExponents) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  descriptor->mutable_sensor()->mutable_input()->emplace_back();
  auto values = descriptor->mutable_sensor()->mutable_input()->back().mutable_values();

  fuchsia::input::report::SensorAxis axis;
  axis.axis.unit.type = fuchsia::input::report::UnitType::SI_LINEAR_VELOCITY;
  axis.axis.unit.exponent = -1;
  axis.axis.range.min = 0;
  axis.axis.range.max = 1000;
  axis.type = fuchsia::input::report::SensorType::ACCELEROMETER_X;

  values->push_back(axis);

  axis.axis.unit.type = fuchsia::input::report::UnitType::LUX;
  axis.axis.unit.exponent = -2;
  axis.type = fuchsia::input::report::SensorType::LIGHT_ILLUMINANCE;

  values->push_back(axis);

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Sensor Descriptor:\n",
      "  Value 00:\n",
      "    SensorType: ACCELEROMETER_X\n",
      "    Unit: SI_LINEAR_VELOCITY * 1e-1\n",
      "    Min:         0\n",
      "    Max:      1000\n",
      "  Value 01:\n",
      "    SensorType: LIGHT_ILLUMINANCE\n",
      "    Unit:      LUX * 1e-2\n",
      "    Min:         0\n",
      "    Max:      1000\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));
  loop_->RunUntilIdle();
}

TEST_F(PrintInputReport, PrintFeatureReport) {
  fuchsia::input::report::FeatureReport report;
  report.mutable_touch()->set_input_mode(
      fuchsia::input::report::TouchConfigurationInputMode::WINDOWS_PRECISION_TOUCHPAD_COLLECTION);
  report.mutable_touch()->mutable_selective_reporting()->set_surface_switch(true);
  report.mutable_touch()->mutable_selective_reporting()->set_button_switch(true);

  std::vector<fuchsia::input::report::FeatureReport> reports;
  reports.push_back(std::move(report));
  fake_device_->SetReports(std::move(reports));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Feature Report from file: test\n",
      "  Touch Feature Report:\n",
      "    Input Mode: 3\n",
      "    Selective Reporting:\n",
      "      Surface Switch: 1\n",
      "      Button Switch: 1\n",
  });

  print_input_report::PrintFeatureReports("test", &printer, std::move(*client_));
  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintDeviceInfo) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  auto device_info = descriptor->mutable_device_information();

  device_info->set_vendor_id(0x1234);
  device_info->set_product_id(0x4321);
  device_info->set_version(0x1111);
  device_info->set_polling_rate(1000);

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Device Info:\n",
      "  Vendor ID:\n",
      "    0x1234\n",
      "  Product ID:\n",
      "    0x4321\n",
      "  Version:\n",
      "    0x1111\n",
      "  Polling Rate:\n",
      "    1000 usec\n",
      "  Manufacturer Name:\n",
      "    None\n",
      "  Product Name:\n",
      "    None\n",
      "  Serial Number:\n",
      "    None\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));

  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, PrintDeviceInfoStrings) {
  auto descriptor = std::make_unique<fuchsia::input::report::DeviceDescriptor>();
  auto device_info = descriptor->mutable_device_information();

  device_info->set_vendor_id(0x1234);
  device_info->set_product_id(0x4321);
  device_info->set_version(0x1111);
  device_info->set_polling_rate(1000);
  device_info->set_manufacturer_name("Google");
  device_info->set_product_name("Fuchsia Device");
  device_info->set_serial_number("1234567890");

  fake_device_->SetDescriptor(std::move(descriptor));

  FakePrinter printer;
  printer.SetExpectedStrings(std::vector<std::string>{
      "Descriptor from file: test\n",
      "Device Info:\n",
      "  Vendor ID:\n",
      "    0x1234\n",
      "  Product ID:\n",
      "    0x4321\n",
      "  Version:\n",
      "    0x1111\n",
      "  Polling Rate:\n",
      "    1000 usec\n",
      "  Manufacturer Name:\n",
      "    Google\n",
      "  Product Name:\n",
      "    Fuchsia Device\n",
      "  Serial Number:\n",
      "    1234567890\n",
  });

  print_input_report::PrintInputDescriptor(std::string("test"), &printer, std::move(*client_));

  loop_->RunUntilIdle();
  printer.AssertSawAllStrings();
}

TEST_F(PrintInputReport, InputReportsReaderV2InitialReports) {
  fuchsia::input::report::InputReport report1;
  report1.mutable_mouse()->set_movement_x(10);
  fuchsia::input::report::InputReport report2;
  report2.mutable_mouse()->set_movement_x(20);

  std::vector<fuchsia::input::report::InputReport> initial_reports;
  initial_reports.push_back(std::move(report1));
  initial_reports.push_back(std::move(report2));
  fake_device_->SetReports(std::move(initial_reports));

  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2;
  bool got_callback = false;
  fake_device_->GetInputReportsReaderV2(reader_v2.NewRequest(loop_->dispatcher()), 5,
                                        [&](uint16_t max_unacknowledged_reports) {
                                          EXPECT_EQ(max_unacknowledged_reports, 5);
                                          got_callback = true;
                                        });

  std::vector<std::vector<fuchsia::input::report::InputReport>> received_batches;
  std::vector<uint64_t> received_stamps;
  reader_v2.events().OnInputReports = [&](std::vector<fuchsia::input::report::InputReport> reports,
                                          uint64_t last_report_stamp) {
    received_batches.push_back(std::move(reports));
    received_stamps.push_back(last_report_stamp);
  };

  loop_->RunUntilIdle();

  EXPECT_TRUE(got_callback);
  ASSERT_EQ(received_batches.size(), 1u);
  ASSERT_EQ(received_batches[0].size(), 2u);
  EXPECT_EQ(received_batches[0][0].mouse().movement_x(), 10);
  EXPECT_EQ(received_batches[0][1].mouse().movement_x(), 20);
  ASSERT_EQ(received_stamps.size(), 1u);
  EXPECT_EQ(received_stamps[0], 2u);
}

TEST_F(PrintInputReport, InputReportsReaderV2AlreadyBound) {
  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2_first;
  fake_device_->GetInputReportsReaderV2(
      reader_v2_first.NewRequest(loop_->dispatcher()), 5,
      [](uint16_t max_unacknowledged_reports) { EXPECT_EQ(max_unacknowledged_reports, 5); });

  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2_second;
  bool got_second_callback = false;
  fake_device_->GetInputReportsReaderV2(reader_v2_second.NewRequest(loop_->dispatcher()), 5,
                                        [&](uint16_t max_unacknowledged_reports) {
                                          EXPECT_EQ(max_unacknowledged_reports, 0);
                                          got_second_callback = true;
                                        });

  zx_status_t second_error = ZX_OK;
  reader_v2_second.set_error_handler([&](zx_status_t status) { second_error = status; });

  loop_->RunUntilIdle();

  EXPECT_TRUE(got_second_callback);
  EXPECT_EQ(second_error, ZX_ERR_ALREADY_BOUND);
}

TEST_F(PrintInputReport, InputReportsReaderV2MonotonicStamps) {
  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2;
  fake_device_->GetInputReportsReaderV2(
      reader_v2.NewRequest(loop_->dispatcher()), 10,
      [](uint16_t max_unacknowledged_reports) { EXPECT_EQ(max_unacknowledged_reports, 10); });

  std::vector<uint64_t> received_stamps;
  reader_v2.events().OnInputReports = [&](std::vector<fuchsia::input::report::InputReport> reports,
                                          uint64_t last_report_stamp) {
    received_stamps.push_back(last_report_stamp);
  };

  // Push first report batch (size 1).
  {
    fuchsia::input::report::InputReport r;
    r.mutable_mouse()->set_movement_x(1);
    std::vector<fuchsia::input::report::InputReport> reports;
    reports.push_back(std::move(r));
    fake_device_->SetReports(std::move(reports));
  }
  loop_->RunUntilIdle();

  // Push second report batch (size 2).
  {
    fuchsia::input::report::InputReport r1;
    r1.mutable_mouse()->set_movement_x(2);
    fuchsia::input::report::InputReport r2;
    r2.mutable_mouse()->set_movement_x(3);
    std::vector<fuchsia::input::report::InputReport> reports;
    reports.push_back(std::move(r1));
    reports.push_back(std::move(r2));
    fake_device_->SetReports(std::move(reports));
  }
  loop_->RunUntilIdle();

  ASSERT_EQ(received_stamps.size(), 2u);
  EXPECT_EQ(received_stamps[0], 1u);
  EXPECT_EQ(received_stamps[1], 3u);
  EXPECT_LT(received_stamps[0], received_stamps[1]);
}

TEST_F(PrintInputReport, InputReportsReaderV2FlowControlAndAcknowledge) {
  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2;
  // Limit unacknowledged reports in flight to 2.
  fake_device_->GetInputReportsReaderV2(
      reader_v2.NewRequest(loop_->dispatcher()), 2,
      [](uint16_t max_unacknowledged_reports) { EXPECT_EQ(max_unacknowledged_reports, 2); });

  std::vector<std::vector<fuchsia::input::report::InputReport>> received_batches;
  std::vector<uint64_t> received_stamps;
  reader_v2.events().OnInputReports = [&](std::vector<fuchsia::input::report::InputReport> reports,
                                          uint64_t last_report_stamp) {
    received_batches.push_back(std::move(reports));
    received_stamps.push_back(last_report_stamp);
  };

  // Push 5 reports at once.
  std::vector<fuchsia::input::report::InputReport> reports;
  for (int i = 0; i < 5; ++i) {
    fuchsia::input::report::InputReport r;
    r.mutable_mouse()->set_movement_x(i);
    reports.push_back(std::move(r));
  }
  fake_device_->SetReports(std::move(reports));

  loop_->RunUntilIdle();

  // Because max_unacknowledged_reports is 2, only 2 reports should be sent initially.
  ASSERT_EQ(received_batches.size(), 1u);
  EXPECT_EQ(received_batches[0].size(), 2u);
  ASSERT_EQ(received_stamps.size(), 1u);
  EXPECT_EQ(received_stamps[0], 2u);

  // Acknowledge the first 2 reports.
  reader_v2->AcknowledgeReports(2);
  loop_->RunUntilIdle();

  // Next 2 reports should be dispatched.
  ASSERT_EQ(received_batches.size(), 2u);
  EXPECT_EQ(received_batches[1].size(), 2u);
  ASSERT_EQ(received_stamps.size(), 2u);
  EXPECT_EQ(received_stamps[1], 4u);

  // Acknowledge up to report stamp 4.
  reader_v2->AcknowledgeReports(4);
  loop_->RunUntilIdle();

  // Final 1 report should be dispatched.
  ASSERT_EQ(received_batches.size(), 3u);
  EXPECT_EQ(received_batches[2].size(), 1u);
  ASSERT_EQ(received_stamps.size(), 3u);
  EXPECT_EQ(received_stamps[2], 5u);
}

TEST_F(PrintInputReport, InputReportsReaderV2ZeroLimitClampedToOne) {
  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2;
  bool got_callback = false;
  fake_device_->GetInputReportsReaderV2(reader_v2.NewRequest(loop_->dispatcher()), 0,
                                        [&](uint16_t max_unacknowledged_reports) {
                                          EXPECT_EQ(max_unacknowledged_reports, 1);
                                          got_callback = true;
                                        });

  std::vector<std::vector<fuchsia::input::report::InputReport>> received_batches;
  std::vector<uint64_t> received_stamps;
  reader_v2.events().OnInputReports = [&](std::vector<fuchsia::input::report::InputReport> reports,
                                          uint64_t last_report_stamp) {
    received_batches.push_back(std::move(reports));
    received_stamps.push_back(last_report_stamp);
  };

  std::vector<fuchsia::input::report::InputReport> reports;
  for (int i = 0; i < 2; ++i) {
    fuchsia::input::report::InputReport r;
    r.mutable_mouse()->set_movement_x(i);
    reports.push_back(std::move(r));
  }
  fake_device_->SetReports(std::move(reports));

  loop_->RunUntilIdle();

  EXPECT_TRUE(got_callback);
  // With clamp to 1, only 1 report is delivered initially.
  ASSERT_EQ(received_batches.size(), 1u);
  EXPECT_EQ(received_batches[0].size(), 1u);
  EXPECT_EQ(received_stamps.size(), 1u);
  EXPECT_EQ(received_stamps[0], 1u);

  // Acknowledge the first report, next should be delivered.
  reader_v2->AcknowledgeReports(1);
  loop_->RunUntilIdle();

  ASSERT_EQ(received_batches.size(), 2u);
  EXPECT_EQ(received_batches[1].size(), 1u);
  EXPECT_EQ(received_stamps.size(), 2u);
  EXPECT_EQ(received_stamps[1], 2u);
}

TEST_F(PrintInputReport, InputReportsReaderV2AcknowledgeEdgeCases) {
  fuchsia::input::report::InputReportsReaderV2Ptr reader_v2;
  fake_device_->GetInputReportsReaderV2(
      reader_v2.NewRequest(loop_->dispatcher()), 1,
      [](uint16_t max_unacknowledged_reports) { EXPECT_EQ(max_unacknowledged_reports, 1); });

  std::vector<std::vector<fuchsia::input::report::InputReport>> received_batches;
  std::vector<uint64_t> received_stamps;
  reader_v2.events().OnInputReports = [&](std::vector<fuchsia::input::report::InputReport> reports,
                                          uint64_t last_report_stamp) {
    received_batches.push_back(std::move(reports));
    received_stamps.push_back(last_report_stamp);
  };

  // Push report 1.
  {
    fuchsia::input::report::InputReport r;
    r.mutable_mouse()->set_movement_x(1);
    std::vector<fuchsia::input::report::InputReport> reports;
    reports.push_back(std::move(r));
    fake_device_->SetReports(std::move(reports));
  }
  loop_->RunUntilIdle();

  ASSERT_EQ(received_batches.size(), 1u);
  EXPECT_EQ(received_stamps[0], 1u);

  // Acknowledge a stamp far in the future (e.g. 100). Should be clamped to current stamp (1).
  reader_v2->AcknowledgeReports(100);
  loop_->RunUntilIdle();

  // Push report 2.
  {
    fuchsia::input::report::InputReport r;
    r.mutable_mouse()->set_movement_x(2);
    std::vector<fuchsia::input::report::InputReport> reports;
    reports.push_back(std::move(r));
    fake_device_->SetReports(std::move(reports));
  }
  loop_->RunUntilIdle();

  // Report 2 should be delivered (since report 1 was acknowledged).
  ASSERT_EQ(received_batches.size(), 2u);
  EXPECT_EQ(received_stamps[1], 2u);

  // Acknowledge an older stamp (e.g. 0). Should not regress acknowledgment state.
  reader_v2->AcknowledgeReports(0);
  loop_->RunUntilIdle();

  // Now acknowledge report 2 properly.
  reader_v2->AcknowledgeReports(2);
  loop_->RunUntilIdle();

  // Push report 3 to verify pipeline is clean.
  {
    fuchsia::input::report::InputReport r;
    r.mutable_mouse()->set_movement_x(3);
    std::vector<fuchsia::input::report::InputReport> reports;
    reports.push_back(std::move(r));
    fake_device_->SetReports(std::move(reports));
  }
  loop_->RunUntilIdle();

  ASSERT_EQ(received_batches.size(), 3u);
  EXPECT_EQ(received_stamps[2], 3u);
}

TEST_F(PrintInputReport, InputReportsReaderV1NotSupported) {
  fuchsia::input::report::InputReportsReaderPtr reader;
  fake_device_->GetInputReportsReader(reader.NewRequest(loop_->dispatcher()));
  zx_status_t reader_error = ZX_OK;
  reader.set_error_handler([&](zx_status_t status) { reader_error = status; });
  loop_->RunUntilIdle();
  EXPECT_EQ(reader_error, ZX_ERR_NOT_SUPPORTED);
}

}  // namespace test
