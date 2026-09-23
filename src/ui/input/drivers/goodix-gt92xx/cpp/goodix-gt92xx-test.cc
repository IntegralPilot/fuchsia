// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "goodix-gt92xx.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/ddk/metadata.h>
#include <lib/hid/gt92xx.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>

#include <gtest/gtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace goodix {

class Gt92xxTestDevice : public Gt92xxDevice {
 public:
  Gt92xxTestDevice(async_dispatcher_t* dispatcher, ddk::I2cChannel i2c,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> intr,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> reset, zx_device_t* parent)
      : Gt92xxDevice(parent, dispatcher, std::move(i2c), std::move(intr), std::move(reset)) {}

  void Running(bool run) { Gt92xxDevice::running_.store(run); }

  zx_status_t Init() { return Gt92xxDevice::Init(); }

  void Trigger() { irq_.trigger(0, zx::time_boot()); }
  zx_status_t StartThread() {
    EXPECT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq_));

    auto thunk = [](void* arg) -> int {
      return reinterpret_cast<Gt92xxTestDevice*>(arg)->Thread();
    };

    Running(true);
    int ret = thrd_create_with_name(&test_thread_, thunk, this, "gt92xx-test-thread");
    return (ret == thrd_success) ? ZX_OK : ZX_ERR_BAD_STATE;
  }
  zx_status_t StopThread() {
    Running(false);
    irq_.trigger(0, zx::time_boot());
    int ret = thrd_join(test_thread_, NULL);
    return (ret == thrd_success) ? ZX_OK : ZX_ERR_BAD_STATE;
  }

  thrd_t test_thread_;
};

class Gt92xxTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(fidl_servers_loop_.StartThread("fidl-servers"));
    fidl::ClientEnd reset_gpio_client = reset_gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
    fidl::ClientEnd intr_gpio_client = intr_gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    fidl::BindServer(fidl_servers_loop_.dispatcher(), std::move(endpoints.server), &mock_i2c_);
    fake_parent_ = MockDevice::FakeRootParent();
    device_.emplace(input_report_loop_.dispatcher(), std::move(endpoints.client),
                    std::move(intr_gpio_client), std::move(reset_gpio_client), fake_parent_.get());
    EXPECT_OK(input_report_loop_.RunUntilIdle());
  }

  void TearDown() override {}

  void InitDevice() {
    ASSERT_OK(device_->Init());

    std::vector reset_states = reset_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(reset_states.size(), 2u);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, reset_states[0].sub_state);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 1}, reset_states[1].sub_state);

    std::vector intr_states = intr_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(intr_states.size(), 2u);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, intr_states[0].sub_state);
    ASSERT_EQ(fake_gpio::ReadSubState{}, intr_states[1].sub_state);
  }

  class ReaderV2EventHandler
      : public fidl::WireAsyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
   public:
    explicit ReaderV2EventHandler(
        fit::function<
            void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
            callback)
        : callback_(std::move(callback)) {}

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

  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> GetReader(
      std::shared_ptr<ReaderV2EventHandler> event_handler) {
    fidl::Endpoints<fuchsia_input_report::InputDevice> endpoints =
        fidl::Endpoints<fuchsia_input_report::InputDevice>::Create();
    fidl::BindServer(input_report_loop_.dispatcher(), std::move(endpoints.server), &*device_);
    input_device_client_.emplace(std::move(endpoints.client), input_report_loop_.dispatcher());

    fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> reader_endpoints =
        fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
    (*input_device_client_)
        ->GetInputReportsReaderV2(std::move(reader_endpoints.server), 10)
        .ThenExactlyOnce(
            [](fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2>&
                   result) {
              zx_status_t status = result.status();
              EXPECT_OK(status);
            });
    EXPECT_OK(input_report_loop_.RunUntilIdle());

    fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader(
        std::move(reader_endpoints.client), input_report_loop_.dispatcher(), event_handler.get());
    EXPECT_OK(input_report_loop_.RunUntilIdle());

    return reader;
  }

  async::Loop fidl_servers_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> reset_gpio_{
      fidl_servers_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> intr_gpio_{
      fidl_servers_loop_.dispatcher(), std::in_place};
  mock_i2c::MockI2cGtest mock_i2c_;
  std::optional<Gt92xxTestDevice> device_;
  std::shared_ptr<MockDevice> fake_parent_;
  std::optional<fidl::WireClient<fuchsia_input_report::InputDevice>> input_device_client_;

  async::Loop input_report_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_F(Gt92xxTest, Init) {
  fbl::Vector<uint8_t> conf_data = Gt92xxDevice::GetConfData();
  mock_i2c_
      .ExpectWrite({static_cast<uint8_t>(GT_REG_CONFIG_DATA >> 8),
                    static_cast<uint8_t>(GT_REG_CONFIG_DATA & 0xff)})
      .ExpectReadStop({0x00})
      .ExpectWriteStop(std::vector<uint8_t>(conf_data.begin(), conf_data.end()))
      .ExpectWriteStop({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                        static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff), 0x00})
      .ExpectWrite({static_cast<uint8_t>(GT_REG_CONFIG_DATA >> 8),
                    static_cast<uint8_t>(GT_REG_CONFIG_DATA & 0xff)})
      .ExpectReadStop({0x00})
      .ExpectWrite({static_cast<uint8_t>(GT_REG_FW_VERSION >> 8),
                    static_cast<uint8_t>(GT_REG_FW_VERSION & 0xff)})
      .ExpectReadStop({0x05, 0x61});

  InitDevice();
}

TEST_F(Gt92xxTest, InitForceConfig) {
  fbl::Vector conf_data = Gt92xxDevice::GetConfData();
  EXPECT_NE(conf_data[sizeof(uint16_t)], 0x00);
  conf_data[sizeof(uint16_t)] = 0x00;

  mock_i2c_
      .ExpectWrite({static_cast<uint8_t>(GT_REG_CONFIG_DATA >> 8),
                    static_cast<uint8_t>(GT_REG_CONFIG_DATA & 0xff)})
      .ExpectReadStop({0x60})
      .ExpectWriteStop(std::vector<uint8_t>(conf_data.begin(), conf_data.end()))
      .ExpectWriteStop({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                        static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff), 0x00})
      .ExpectWrite({static_cast<uint8_t>(GT_REG_CONFIG_DATA >> 8),
                    static_cast<uint8_t>(GT_REG_CONFIG_DATA & 0xff)})
      .ExpectReadStop({0x60})
      .ExpectWrite({static_cast<uint8_t>(GT_REG_FW_VERSION >> 8),
                    static_cast<uint8_t>(GT_REG_FW_VERSION & 0xff)})
      .ExpectReadStop({0x05, 0x61});

  InitDevice();
}

TEST_F(Gt92xxTest, TestGetDescriptor) {
  ASSERT_OK(input_report_loop_.StartThread("input-report-loop"));

  auto endpoints = fidl::Endpoints<fuchsia_input_report::InputDevice>::Create();
  fidl::BindServer(input_report_loop_.dispatcher(), std::move(endpoints.server), &*device_);
  fidl::WireSyncClient<fuchsia_input_report::InputDevice> client(std::move(endpoints.client));

  auto result = client->GetDescriptor();
  EXPECT_TRUE(result.ok());
  auto descriptor = result->descriptor;

  EXPECT_TRUE(descriptor.has_device_information());
  EXPECT_EQ(descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(
      descriptor.device_information().product_id(),
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kGoodixTouchscreen));

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
  EXPECT_EQ(descriptor.touch().input().max_contacts(), 5u);

  EXPECT_FALSE(descriptor.touch().input().has_buttons());
  EXPECT_TRUE(descriptor.touch().input().has_contacts());
  EXPECT_EQ(descriptor.touch().input().contacts().size(), 5u);

  for (const auto& c : descriptor.touch().input().contacts()) {
    EXPECT_TRUE(c.has_position_x());
    EXPECT_TRUE(c.has_position_y());
    EXPECT_FALSE(c.has_contact_height());
    EXPECT_FALSE(c.has_contact_width());
    EXPECT_FALSE(c.has_pressure());

    EXPECT_EQ(c.position_x().range.min, 0);
    EXPECT_EQ(c.position_x().range.max, 600);
    EXPECT_EQ(c.position_x().unit.type, fuchsia_input_report::wire::UnitType::kOther);
    EXPECT_EQ(c.position_x().unit.exponent, 0);

    EXPECT_EQ(c.position_y().range.min, 0);
    EXPECT_EQ(c.position_y().range.max, 1024);
    EXPECT_EQ(c.position_y().unit.type, fuchsia_input_report::wire::UnitType::kOther);
    EXPECT_EQ(c.position_y().unit.exponent, 0);
  }
}

TEST_F(Gt92xxTest, TestReport) {
  mock_i2c_
      .ExpectWrite({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                    static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff)})
      .ExpectReadStop({0x85})
      .ExpectWrite(
          {static_cast<uint8_t>(GT_REG_REPORTS >> 8), static_cast<uint8_t>(GT_REG_REPORTS & 0xff)})
      .ExpectReadStop({0x00, 0x00, 0x01, 0x10, 0x01, 0x01, 0x01, 0x00, 0x01, 0x00,
                       0x02, 0x20, 0x02, 0x01, 0x01, 0x00, 0x02, 0x00, 0x03, 0x30,
                       0x03, 0x01, 0x01, 0x00, 0x03, 0x00, 0x04, 0x40, 0x04, 0x01,
                       0x01, 0x00, 0x04, 0x00, 0x05, 0x50, 0x05, 0x01, 0x01, 0x00})
      .ExpectWriteStop({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                        static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff), 0x00});
  EXPECT_OK(device_->StartThread());
  zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));

  std::shared_ptr<ReaderV2EventHandler> event_handler = std::make_shared<ReaderV2EventHandler>(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 5u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(), 0x110);
        EXPECT_EQ(touch_report.contacts()[0].position_y(), 0x100);

        EXPECT_EQ(touch_report.contacts()[1].contact_id(), 1u);
        EXPECT_EQ(touch_report.contacts()[1].position_x(), 0x220);
        EXPECT_EQ(touch_report.contacts()[1].position_y(), 0x200);

        EXPECT_EQ(touch_report.contacts()[2].contact_id(), 2u);
        EXPECT_EQ(touch_report.contacts()[2].position_x(), 0x330);
        EXPECT_EQ(touch_report.contacts()[2].position_y(), 0x300);

        EXPECT_EQ(touch_report.contacts()[3].contact_id(), 3u);
        EXPECT_EQ(touch_report.contacts()[3].position_x(), 0x440);
        EXPECT_EQ(touch_report.contacts()[3].position_y(), 0x400);

        EXPECT_EQ(touch_report.contacts()[4].contact_id(), 4u);
        EXPECT_EQ(touch_report.contacts()[4].position_x(), 0x550);
        EXPECT_EQ(touch_report.contacts()[4].position_y(), 0x500);

        input_report_loop_.Quit();
      });

  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader(event_handler);

  device_->Trigger();
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);

  EXPECT_OK(device_->StopThread());
}

TEST_F(Gt92xxTest, TestReportMoreContacts) {
  mock_i2c_
      .ExpectWrite({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                    static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff)})
      .ExpectReadStop({0x87})
      .ExpectWrite(
          {static_cast<uint8_t>(GT_REG_REPORTS >> 8), static_cast<uint8_t>(GT_REG_REPORTS & 0xff)})
      .ExpectReadStop({0x00, 0x00, 0x01, 0x10, 0x01, 0x01, 0x01, 0x00, 0x01, 0x00,
                       0x02, 0x20, 0x02, 0x01, 0x01, 0x00, 0x02, 0x00, 0x03, 0x30,
                       0x03, 0x01, 0x01, 0x00, 0x03, 0x00, 0x04, 0x40, 0x04, 0x01,
                       0x01, 0x00, 0x04, 0x00, 0x05, 0x50, 0x05, 0x01, 0x01, 0x00})
      .ExpectWriteStop({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                        static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff), 0x00});
  EXPECT_OK(device_->StartThread());
  zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));

  std::shared_ptr<ReaderV2EventHandler> event_handler = std::make_shared<ReaderV2EventHandler>(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 5u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(), 0x110);
        EXPECT_EQ(touch_report.contacts()[0].position_y(), 0x100);

        EXPECT_EQ(touch_report.contacts()[1].contact_id(), 1u);
        EXPECT_EQ(touch_report.contacts()[1].position_x(), 0x220);
        EXPECT_EQ(touch_report.contacts()[1].position_y(), 0x200);

        EXPECT_EQ(touch_report.contacts()[2].contact_id(), 2u);
        EXPECT_EQ(touch_report.contacts()[2].position_x(), 0x330);
        EXPECT_EQ(touch_report.contacts()[2].position_y(), 0x300);

        EXPECT_EQ(touch_report.contacts()[3].contact_id(), 3u);
        EXPECT_EQ(touch_report.contacts()[3].position_x(), 0x440);
        EXPECT_EQ(touch_report.contacts()[3].position_y(), 0x400);

        EXPECT_EQ(touch_report.contacts()[4].contact_id(), 4u);
        EXPECT_EQ(touch_report.contacts()[4].position_x(), 0x550);
        EXPECT_EQ(touch_report.contacts()[4].position_y(), 0x500);

        input_report_loop_.Quit();
      });

  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader(event_handler);

  device_->Trigger();
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);

  EXPECT_OK(device_->StopThread());
}

TEST_F(Gt92xxTest, TestReportLessContacts) {
  mock_i2c_
      .ExpectWrite({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                    static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff)})
      .ExpectReadStop({0x83})
      .ExpectWrite(
          {static_cast<uint8_t>(GT_REG_REPORTS >> 8), static_cast<uint8_t>(GT_REG_REPORTS & 0xff)})
      .ExpectReadStop({0x00, 0x00, 0x01, 0x10, 0x01, 0x01, 0x01, 0x00, 0x01, 0x00,
                       0x02, 0x20, 0x02, 0x01, 0x01, 0x00, 0x02, 0x00, 0x03, 0x30,
                       0x03, 0x01, 0x01, 0x00, 0x03, 0x00, 0x04, 0x40, 0x04, 0x01,
                       0x01, 0x00, 0x04, 0x00, 0x05, 0x50, 0x05, 0x01, 0x01, 0x00})
      .ExpectWriteStop({static_cast<uint8_t>(GT_REG_TOUCH_STATUS >> 8),
                        static_cast<uint8_t>(GT_REG_TOUCH_STATUS & 0xff), 0x00});
  EXPECT_OK(device_->StartThread());
  zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));

  std::shared_ptr<ReaderV2EventHandler> event_handler = std::make_shared<ReaderV2EventHandler>(
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_touch());
        const fuchsia_input_report::wire::TouchInputReport& touch_report = report.touch();

        ASSERT_TRUE(touch_report.has_contacts());
        ASSERT_EQ(touch_report.contacts().size(), 3u);
        EXPECT_EQ(touch_report.contacts()[0].contact_id(), 0u);
        EXPECT_EQ(touch_report.contacts()[0].position_x(), 0x110);
        EXPECT_EQ(touch_report.contacts()[0].position_y(), 0x100);

        EXPECT_EQ(touch_report.contacts()[1].contact_id(), 1u);
        EXPECT_EQ(touch_report.contacts()[1].position_x(), 0x220);
        EXPECT_EQ(touch_report.contacts()[1].position_y(), 0x200);

        EXPECT_EQ(touch_report.contacts()[2].contact_id(), 2u);
        EXPECT_EQ(touch_report.contacts()[2].position_x(), 0x330);
        EXPECT_EQ(touch_report.contacts()[2].position_y(), 0x300);

        input_report_loop_.Quit();
      });

  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader(event_handler);

  device_->Trigger();
  EXPECT_EQ(input_report_loop_.Run(), ZX_ERR_CANCELED);

  EXPECT_OK(device_->StopThread());
}

}  // namespace goodix
