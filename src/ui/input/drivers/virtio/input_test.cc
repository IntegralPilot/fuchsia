// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fit/defer.h>

#include <set>

#include <virtio/input.h>
#include <zxtest/zxtest.h>

#include "input_kbd.h"
#include "input_mouse.h"
#include "input_touch.h"

#define CHECK_AXIS(axis, name, range_min, range_max, unit_type, uint_exponent) \
  ASSERT_TRUE((axis).has_##name());                                            \
  EXPECT_EQ((axis).name().range.min, (range_min));                             \
  EXPECT_EQ((axis).name().range.max, (range_max));                             \
  EXPECT_EQ((axis).name().unit.type, (unit_type));                             \
  EXPECT_EQ((axis).name().unit.exponent, (uint_exponent));

namespace virtio {

namespace {

void SendTouchEvent(async_patterns::TestDispatcherBound<HidTouch>& touch, uint16_t type,
                    uint16_t code, uint32_t value) {
  virtio_input_event_t event = {};
  event.type = type;
  event.code = code;
  event.value = value;

  touch.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });
}

}  // namespace

class VirtioInputTest : public zxtest::Test {
 public:
  VirtioInputTest()
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        input_report_loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

  void SetUp() override { loop_.StartThread("virtio-input-test-loop"); }

  class ReaderV2EventHandler
      : public fidl::WireAsyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
   public:
    explicit ReaderV2EventHandler(
        fit::function<
            void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
            callback)
        : callback_(std::move(callback)) {}

    void SetCallback(
        fit::function<
            void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
            callback) {
      callback_ = std::move(callback);
    }

    void OnInputReports(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*
                            event) override {
      if (callback_) {
        callback_(event);
      }
    }

    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

   private:
    fit::function<void(
        fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
        callback_;
  };

  template <typename HidDeviceType>
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> GetReader(
      HidDeviceType& device, std::shared_ptr<ReaderV2EventHandler> event_handler) {
    fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> reader_endpoints =
        fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();

    device.SyncCall([&](HidDeviceBase* device) {
      EXPECT_EQ(ZX_OK, device->GetInputReportsReaderV2(async_get_default_dispatcher(),
                                                       std::move(reader_endpoints.server), 50));
    });
    fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader(
        std::move(reader_endpoints.client), input_report_loop_.dispatcher(), event_handler.get());
    EXPECT_OK(input_report_loop_.RunUntilIdle());

    return reader;
  }

 protected:
  async::Loop loop_;
  async::Loop input_report_loop_;
};

TEST_F(VirtioInputTest, MultiTouchReportDescriptor) {
  virtio_input_absinfo_t x_info = {};
  virtio_input_absinfo_t y_info = {};
  HidTouch touch(x_info, y_info, std::string("test_product"), std::string("test_serial"));

  fidl::Arena<> allocator;
  auto descriptor = touch.GetDescriptor(allocator);

  // Assert that the report descriptor is correct.
  EXPECT_FALSE(descriptor.has_consumer_control());
  EXPECT_FALSE(descriptor.has_keyboard());
  EXPECT_FALSE(descriptor.has_mouse());
  EXPECT_FALSE(descriptor.has_sensor());

  ASSERT_TRUE(descriptor.has_device_information());
  EXPECT_EQ(descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(
      descriptor.device_information().product_id(),
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kVirtioTouchscreen));

  ASSERT_TRUE(descriptor.has_touch());
  EXPECT_FALSE(descriptor.touch().has_feature());
  ASSERT_TRUE(descriptor.touch().has_input());
  ASSERT_TRUE(descriptor.touch().input().has_touch_type());
  EXPECT_EQ(descriptor.touch().input().touch_type(), fuchsia_input_report::TouchType::kTouchscreen);
  EXPECT_FALSE(descriptor.touch().input().has_buttons());
  ASSERT_TRUE(descriptor.touch().input().has_contacts());
  ASSERT_EQ(descriptor.touch().input().contacts().size(), kMaxTouchPoints);
  for (const auto& contact : descriptor.touch().input().contacts()) {
    EXPECT_FALSE(contact.has_contact_height());
    EXPECT_FALSE(contact.has_contact_width());
    EXPECT_FALSE(contact.has_pressure());

    CHECK_AXIS(contact, position_x, 0L, 259200L, fuchsia_input_report::wire::UnitType::kMeters, -6)
    CHECK_AXIS(contact, position_y, 0L, 172800L, fuchsia_input_report::wire::UnitType::kMeters, -6)
  }
}

TEST_F(VirtioInputTest, MultiTouchFingerEvents) {
  constexpr int VAL_MAX = 100;
  constexpr int X_VAL = 50;
  constexpr int Y_VAL = 100;
  virtio_input_absinfo_t x_info = {};
  x_info.min = 0;
  x_info.max = static_cast<uint16_t>(VAL_MAX);
  virtio_input_absinfo_t y_info = {};
  y_info.min = 0;
  y_info.max = static_cast<uint16_t>(VAL_MAX);
  async_patterns::TestDispatcherBound<HidTouch> touch{
      loop_.dispatcher(),        std::in_place, x_info, y_info, std::string{"test_product"},
      std::string{"test_serial"}};
  std::shared_ptr<ReaderV2EventHandler> event_handler =
      std::make_shared<ReaderV2EventHandler>(nullptr);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader =
      GetReader(touch, event_handler);

  // Assert that a single finger works.
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_SLOT, 0);
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_TRACKING_ID, 1);
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_POSITION_X,
                 static_cast<uint16_t>(X_VAL));
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_POSITION_Y,
                 static_cast<uint16_t>(Y_VAL));

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 1u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(),
                  X_VAL * HidTouch::kXPhysicalMaxMicrometer / VAL_MAX);
        EXPECT_EQ(touch_report.contacts()[0].position_y(),
                  Y_VAL * HidTouch::kYPhysicalMaxMicrometer / VAL_MAX);

        input_report_loop_.Quit();
      });

  touch.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // Assert that a second finger tracks.
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_SLOT, 1);
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_TRACKING_ID, 2);

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 2u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(),
                  X_VAL * HidTouch::kXPhysicalMaxMicrometer / VAL_MAX);
        EXPECT_EQ(touch_report.contacts()[0].position_y(),
                  Y_VAL * HidTouch::kYPhysicalMaxMicrometer / VAL_MAX);
        EXPECT_EQ(touch_report.contacts()[1].contact_id(), 1u);

        input_report_loop_.Quit();
      });

  touch.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // Pick up the second finger.

  // We don't send another SLOT event because we will rely on the slot already
  // being 1.
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_TRACKING_ID, -1);

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 1u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(),
                  X_VAL * HidTouch::kXPhysicalMaxMicrometer / VAL_MAX);
        EXPECT_EQ(touch_report.contacts()[0].position_y(),
                  Y_VAL * HidTouch::kYPhysicalMaxMicrometer / VAL_MAX);

        input_report_loop_.Quit();
      });

  touch.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // Out of bounds slot. GetReport values shouldn't change.
  int NEW_X_VAL = 100;
  int NEW_Y_VAL = 50;
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_SLOT, 5);
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_TRACKING_ID, 1);
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_POSITION_X,
                 static_cast<uint16_t>(NEW_X_VAL));
  SendTouchEvent(touch, VIRTIO_INPUT_EV_ABS, VIRTIO_INPUT_EV_MT_POSITION_Y,
                 static_cast<uint16_t>(NEW_Y_VAL));

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 1u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(),
                  X_VAL * HidTouch::kXPhysicalMaxMicrometer / VAL_MAX);
        EXPECT_EQ(touch_report.contacts()[0].position_y(),
                  Y_VAL * HidTouch::kYPhysicalMaxMicrometer / VAL_MAX);

        input_report_loop_.Quit();
      });

  touch.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
}

TEST_F(VirtioInputTest, MouseReportDescriptor) {
  HidMouse hid_mouse(std::string("test_product"), std::string("test_serial"));
  fidl::Arena<> allocator;
  auto descriptor = hid_mouse.GetDescriptor(allocator);

  // Assert that the report descriptor is correct.
  EXPECT_FALSE(descriptor.has_consumer_control());
  EXPECT_FALSE(descriptor.has_keyboard());
  EXPECT_FALSE(descriptor.has_sensor());
  EXPECT_FALSE(descriptor.has_touch());

  ASSERT_TRUE(descriptor.has_device_information());
  EXPECT_EQ(descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(descriptor.device_information().product_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kVirtioMouse));

  ASSERT_TRUE(descriptor.has_mouse());
  ASSERT_TRUE(descriptor.mouse().has_input());
  EXPECT_FALSE(descriptor.mouse().input().has_position_x());
  EXPECT_FALSE(descriptor.mouse().input().has_position_y());
  EXPECT_FALSE(descriptor.mouse().input().has_scroll_h());
  ASSERT_TRUE(descriptor.mouse().input().has_buttons());
  const std::set<uint8_t> kExpectedButtons = {1, 2, 3};
  EXPECT_TRUE(std::set(descriptor.mouse().input().buttons().begin(),
                       descriptor.mouse().input().buttons().end()) == kExpectedButtons);
  CHECK_AXIS(descriptor.mouse().input(), movement_x, -32767L, 32767L,
             fuchsia_input_report::wire::UnitType::kNone, 0)
  CHECK_AXIS(descriptor.mouse().input(), movement_y, -32767L, 32767L,
             fuchsia_input_report::wire::UnitType::kNone, 0)
  CHECK_AXIS(descriptor.mouse().input(), scroll_v, -32767L, 32767L,
             fuchsia_input_report::wire::UnitType::kNone, 0)
}

TEST_F(VirtioInputTest, MouseTest) {
  async_patterns::TestDispatcherBound<HidMouse> hid_mouse{
      loop_.dispatcher(), std::in_place, std::string("test_product"), std::string("test_serial")};
  std::shared_ptr<ReaderV2EventHandler> event_handler =
      std::make_shared<ReaderV2EventHandler>(nullptr);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader =
      GetReader(hid_mouse, event_handler);

  // Send the Virtio mouse keys.
  virtio_input_event_t event = {};
  event.type = VIRTIO_INPUT_EV_KEY;
  event.value = VIRTIO_INPUT_EV_KEY_PRESSED;
  event.code = 0x110;  // BTN_LEFT.
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_pressed_buttons());
        ASSERT_EQ(mouse_report.pressed_buttons().size(), 1u);
        EXPECT_EQ(mouse_report.pressed_buttons()[0], 1);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // ------------------------------------------------------------------

  // Send another Virtio mouse key event.
  event.type = VIRTIO_INPUT_EV_KEY;
  event.value = VIRTIO_INPUT_EV_KEY_PRESSED;
  event.code = 0x111;  // BTN_RIGHT.
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_pressed_buttons());
        ASSERT_EQ(mouse_report.pressed_buttons().size(), 2u);
        EXPECT_EQ(mouse_report.pressed_buttons()[0], 1);
        EXPECT_EQ(mouse_report.pressed_buttons()[1], 2);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // ------------------------------------------------------------------

  // Release one Virtio mouse key.
  event.type = VIRTIO_INPUT_EV_KEY;
  event.value = VIRTIO_INPUT_EV_KEY_RELEASED;
  event.code = 0x110;  // BTN_LEFT.
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_pressed_buttons());
        ASSERT_EQ(mouse_report.pressed_buttons().size(), 1u);
        EXPECT_EQ(mouse_report.pressed_buttons()[0], 2);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // ------------------------------------------------------------------

  // Send another Virtio mouse key event.
  event.type = VIRTIO_INPUT_EV_KEY;
  event.value = VIRTIO_INPUT_EV_KEY_PRESSED;
  event.code = 0x112;  // BTN_MID.
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_pressed_buttons());
        ASSERT_EQ(mouse_report.pressed_buttons().size(), 2u);
        EXPECT_EQ(mouse_report.pressed_buttons()[0], 2);
        EXPECT_EQ(mouse_report.pressed_buttons()[1], 3);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // ------------------------------------------------------------------

  // Send a Virtio mouse rel event on X axis.
  event.type = VIRTIO_INPUT_EV_REL;
  event.code = VIRTIO_INPUT_EV_REL_X;
  event.value = 0x0abc;
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_movement_x());
        EXPECT_EQ(mouse_report.movement_x(), 0x0abc);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // ------------------------------------------------------------------

  // Send a Virtio mouse rel event on Y axis.
  event.type = VIRTIO_INPUT_EV_REL;
  event.code = VIRTIO_INPUT_EV_REL_Y;
  event.value = 0x0123;
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_movement_y());
        EXPECT_EQ(mouse_report.movement_y(), 0x0123);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
  EXPECT_OK(input_report_loop_.ResetQuit());

  // ------------------------------------------------------------------

  // Send a Virtio mouse rel event on wheel.
  event.type = VIRTIO_INPUT_EV_REL;
  event.code = VIRTIO_INPUT_EV_REL_WHEEL;
  event.value = 0x0345;
  hid_mouse.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event_handler->SetCallback(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_scroll_v());
        EXPECT_EQ(mouse_report.scroll_v(), 0x0345);

        input_report_loop_.Quit();
      });

  hid_mouse.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
}

TEST_F(VirtioInputTest, KeyboardReportDescriptor) {
  HidKeyboard hid_keyboard(std::string("test_product"), std::string("test_serial"));
  fidl::Arena<> allocator;
  auto descriptor = hid_keyboard.GetDescriptor(allocator);

  // Assert that the report descriptor is correct.
  EXPECT_FALSE(descriptor.has_consumer_control());
  EXPECT_FALSE(descriptor.has_mouse());
  EXPECT_FALSE(descriptor.has_sensor());
  EXPECT_FALSE(descriptor.has_touch());

  ASSERT_TRUE(descriptor.has_device_information());
  EXPECT_EQ(descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(
      descriptor.device_information().product_id(),
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kVirtioKeyboard));

  ASSERT_TRUE(descriptor.has_keyboard());
  EXPECT_FALSE(descriptor.keyboard().has_output());
  ASSERT_TRUE(descriptor.keyboard().has_input());
  ASSERT_TRUE(descriptor.keyboard().input().has_keys3());
  std::set<fuchsia_input::Key> kExpectedKeys;
  for (auto k = fuchsia_input::Key::kA; k < fuchsia_input::Key::kNonUsBackslash;
       k = static_cast<fuchsia_input::Key>(static_cast<uint32_t>(k) + 1)) {
    kExpectedKeys.insert(k);
  }
  kExpectedKeys.erase(fuchsia_input::Key::kNonUsHash);
  kExpectedKeys.erase(fuchsia_input::Key::kPrintScreen);
  kExpectedKeys.erase(fuchsia_input::Key::kKeypadSlash);
  kExpectedKeys.erase(fuchsia_input::Key::kKeypadEnter);
  for (auto k = fuchsia_input::Key::kLeftCtrl; k <= fuchsia_input::Key::kRightMeta;
       k = static_cast<fuchsia_input::Key>(static_cast<uint32_t>(k) + 1)) {
    kExpectedKeys.insert(k);
  }
  kExpectedKeys.insert(fuchsia_input::Key::kMute);
  kExpectedKeys.insert(fuchsia_input::Key::kVolumeDown);
  kExpectedKeys.insert(fuchsia_input::Key::kVolumeUp);
  kExpectedKeys.insert(fuchsia_input::Key::kPower);
  EXPECT_TRUE(std::set(descriptor.keyboard().input().keys3().begin(),
                       descriptor.keyboard().input().keys3().end()) == kExpectedKeys);
}

TEST_F(VirtioInputTest, KeyboardTest) {
  async_patterns::TestDispatcherBound<HidKeyboard> hid_keyboard{
      loop_.dispatcher(), std::in_place, std::string("test_product"), std::string("test_serial")};
  std::shared_ptr<ReaderV2EventHandler> event_handler = std::make_shared<ReaderV2EventHandler>(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_keyboard());
        const fuchsia_input_report::wire::KeyboardInputReport& keyboard_report = report.keyboard();

        ASSERT_TRUE(keyboard_report.has_pressed_keys3());
        ASSERT_EQ(keyboard_report.pressed_keys3().size(), 4u);
        EXPECT_EQ(keyboard_report.pressed_keys3()[0], fuchsia_input::wire::Key::kLeftShift);
        EXPECT_EQ(keyboard_report.pressed_keys3()[1], fuchsia_input::wire::Key::kA);
        EXPECT_EQ(keyboard_report.pressed_keys3()[2], fuchsia_input::wire::Key::kRightAlt);
        EXPECT_EQ(keyboard_report.pressed_keys3()[3], fuchsia_input::wire::Key::kDown);

        input_report_loop_.Quit();
      });
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader =
      GetReader(hid_keyboard, event_handler);

  // Send the Virtio keys.
  virtio_input_event_t event = {};
  event.type = VIRTIO_INPUT_EV_KEY;
  event.value = VIRTIO_INPUT_EV_KEY_PRESSED;
  event.code = 42;  // LEFTSHIFT.
  hid_keyboard.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event.code = 30;  // KEY_A
  hid_keyboard.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event.code = 100;  // KEY_RIGHTALT
  hid_keyboard.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  event.code = 108;  // KEY_DOWN
  hid_keyboard.SyncCall([&](HidDeviceBase* device) { device->ReceiveEvent(&event); });

  hid_keyboard.SyncCall(&HidDeviceBase::SendReportToAllReaders);
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);
}

}  // namespace virtio
