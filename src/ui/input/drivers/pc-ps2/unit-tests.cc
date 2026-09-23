// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/hid/boot.h>
#include <lib/sync/completion.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>
#include <zircon/compiler.h>

#include <condition_variable>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/ui/input/drivers/pc-ps2/commands.h"
#include "src/ui/input/drivers/pc-ps2/controller.h"
#include "src/ui/input/drivers/pc-ps2/device.h"
#include "src/ui/input/drivers/pc-ps2/keymap.h"
#include "src/ui/input/drivers/pc-ps2/registers.h"

class Fake8042;
namespace {
Fake8042* FAKE_INSTANCE = nullptr;
}
class Fake8042 {
 public:
  explicit Fake8042() {
    status_.set_reg_value(0);
    ctrl_.set_reg_value(0);
    zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &port1_irq_);
    zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &port2_irq_);
    FAKE_INSTANCE = this;
  }
  ~Fake8042() { FAKE_INSTANCE = nullptr; }
  uint8_t inp(uint16_t port) {
    switch (port) {
      case i8042::kStatusReg:
        return status_.reg_value();
      case i8042::kDataReg: {
        ZX_ASSERT(status_.obf());
        uint8_t value = data_.front();
        data_.pop_front();
        status_.set_obf(!data_.empty());
        return value;
      }
      default:
        ZX_ASSERT_MSG(false, "Unexpected register 0x%x", port);
    }
  }
  void outp(uint16_t port, uint8_t data) {
    switch (port) {
      case i8042::kCommandReg:
        HandleCommand(data);
        break;
      case i8042::kDataReg:
        HandleData(data);
        break;
    }
  }

  void EnablePort2() {
    has_port2_ = true;
    ctrl_.set_auxdis(1);
  }

  void SendDataAndIrq(bool port2, uint8_t byte) {
    SendData(byte);
    if (port2) {
      port2_irq_.trigger(0, zx::clock::get_boot());
    } else {
      port1_irq_.trigger(0, zx::clock::get_boot());
    }
  }

  void SendData(uint8_t data) {
    data_.emplace_back(data);
    status_.set_obf(1);
  }

  zx::interrupt& port1_irq() { return port1_irq_; }
  zx::interrupt& port2_irq() { return port2_irq_; }

 private:
  enum State {
    // Default state (next write goes to port 1).
    kPort1Write = 0,
    // Next write goes to port 2.
    kPort2Write = 1,
    // Next write goes to control register.
    kControlWrite = 2,
  };

  i8042::StatusReg status_;
  i8042::ControlReg ctrl_;
  State data_state_ = State::kPort1Write;
  bool has_port2_ = false;
  std::list<uint8_t> data_;
  zx::interrupt port1_irq_;
  zx::interrupt port2_irq_;

  void HandleCommand(uint8_t cmd) {
    switch (cmd) {
      case i8042::kCmdReadCtl.cmd:
        SendData(ctrl_.reg_value());
        break;
      case i8042::kCmdWriteCtl.cmd:
        data_state_ = State::kControlWrite;
        break;
      case i8042::kCmdSelfTest.cmd:
        SendData(0x55);
        break;
      case i8042::kCmdWriteAux.cmd:
        ZX_ASSERT(has_port2_);
        data_state_ = State::kPort2Write;
        break;
      case i8042::kCmdPort1Disable.cmd:
      case i8042::kCmdPort1Enable.cmd:
      case i8042::kCmdPort2Disable.cmd:
      case i8042::kCmdPort2Enable.cmd:
        break;

      case i8042::kCmdPort2Test.cmd:
        ZX_ASSERT(has_port2_);
        __FALLTHROUGH;
      case i8042::kCmdPort1Test.cmd:
        SendData(0x00);
        break;
      default:
        ZX_ASSERT_MSG(false, "Unknown command");
    }
  }

  void HandleData(uint8_t data) {
    switch (data_state_) {
      case State::kControlWrite:
        ctrl_.set_reg_value(data);
        break;
      case State::kPort1Write:
        HandleDeviceCommand(false, data);
        break;
      case State::kPort2Write:
        HandleDeviceCommand(true, data);
        break;
    }
    data_state_ = State::kPort1Write;
  }

  void HandleDeviceCommand(bool is_port2, uint8_t command) {
    switch (command) {
      case i8042::kCmdDeviceIdentify.cmd:
        SendData(i8042::kAck);
        SendData(is_port2 ? 0x00 : 0xab);
        break;
      case i8042::kCmdDeviceScanDisable.cmd:
      case i8042::kCmdDeviceScanEnable.cmd:
        SendData(i8042::kAck);
        break;
    }
  }
};

uint8_t TEST_inp(uint16_t port) { return FAKE_INSTANCE->inp(port); }
void TEST_outp(uint16_t port, uint8_t data) { FAKE_INSTANCE->outp(port, data); }
zx::interrupt GetInterrupt(uint32_t irq_no) {
  zx::interrupt* irq = nullptr;
  if (irq_no == 0x1) {
    irq = &FAKE_INSTANCE->port1_irq();
  } else if (irq_no == 0xc) {
    irq = &FAKE_INSTANCE->port2_irq();
  } else {
    ZX_ASSERT_MSG(false, "unexpected irq_no 0x%x", irq_no);
  }

  zx::interrupt clone;
  ZX_ASSERT(irq->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
  return clone;
}

class ControllerTest : public zxtest::Test {
 public:
  void SetUp() override {
    zx_status_t status = i8042::Controller::Bind(nullptr, root_.get());
    ASSERT_OK(status);

    controller_dev_ = root_->GetLatestChild();
  }

  void TearDown() override {
    auto result = fdf::RunOnDispatcherSync(dispatcher_->async_dispatcher(), [&]() {
      device_async_remove(controller_dev_);
      mock_ddk::ReleaseFlaggedDevices(controller_dev_);
    });
    EXPECT_OK(result.status_value());
  }

  void InitDevices() {
    auto result = fdf::RunOnDispatcherSync(dispatcher_->async_dispatcher(),
                                           [&]() { controller_dev_->InitOp(); });
    EXPECT_OK(result.status_value());
    controller_dev_->WaitUntilInitReplyCalled(zx::time::infinite());
    ASSERT_OK(controller_dev_->InitReplyCallStatus());
    sync_completion_wait(&controller_dev_->GetDeviceContext<i8042::Controller>()->added_children(),
                         ZX_TIME_INFINITE);

    auto endpoints = fidl::Endpoints<fuchsia_input_report::InputDevice>::Create();

    binding_ =
        fidl::BindServer(dispatcher_->async_dispatcher(), std::move(endpoints.server),
                         controller_dev_->GetLatestChild()->GetDeviceContext<i8042::I8042Device>());
    client_.Bind(std::move(endpoints.client));
  }

  class SyncReaderV2EventHandler
      : public fidl::WireSyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
   public:
    explicit SyncReaderV2EventHandler(
        fit::function<
            void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
            callback)
        : callback_(std::move(callback)) {}

    void OnInputReports(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*
                            event) override {
      last_report_stamp = event->last_report_stamp;
      if (callback_) {
        callback_(event);
      }
    }

    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

    uint64_t last_report_stamp = 0;

   private:
    fit::function<void(
        fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
        callback_;
  };

  static zx_status_t ReadAndAcknowledgeOneEvent(
      fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>& reader,
      fit::function<
          void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
          callback) {
    SyncReaderV2EventHandler handler(std::move(callback));
    fidl::Status result = reader.HandleOneEvent(handler);
    if (!result.ok()) {
      return result.status();
    }
    return reader->AcknowledgeReports(handler.last_report_stamp).status();
  }

 protected:
  Fake8042 i8042_;
  std::shared_ptr<MockDevice> root_ = MockDevice::FakeRootParent();
  fdf::UnownedSynchronizedDispatcher dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
  zx_device* controller_dev_;

  fidl::WireSyncClient<fuchsia_input_report::InputDevice> client_;

 private:
  std::optional<fidl::ServerBindingRef<fuchsia_input_report::InputDevice>> binding_;
};

TEST_F(ControllerTest, GetKbdDescriptorTest) {
  InitDevices();

  auto response = client_->GetDescriptor();

  ASSERT_TRUE(response.ok());
  ASSERT_TRUE(response.value().descriptor.has_device_information());
  EXPECT_EQ(response.value().descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(
      response.value().descriptor.device_information().product_id(),
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kPcPs2Keyboard));

  ASSERT_TRUE(response.value().descriptor.has_keyboard());
  ASSERT_TRUE(response.value().descriptor.keyboard().has_input());
  ASSERT_TRUE(response.value().descriptor.keyboard().input().has_keys3());
  ASSERT_EQ(response.value().descriptor.keyboard().input().keys3().size(), 106);

  ASSERT_TRUE(response.value().descriptor.keyboard().has_output());
  ASSERT_TRUE(response.value().descriptor.keyboard().output().has_leds());
  ASSERT_EQ(response.value().descriptor.keyboard().output().leds().size(), 5);
  const auto& leds = response.value().descriptor.keyboard().output().leds();
  EXPECT_EQ(leds[0], fuchsia_input_report::wire::LedType::kNumLock);
  EXPECT_EQ(leds[1], fuchsia_input_report::wire::LedType::kCapsLock);
  EXPECT_EQ(leds[2], fuchsia_input_report::wire::LedType::kScrollLock);
  EXPECT_EQ(leds[3], fuchsia_input_report::wire::LedType::kCompose);
  EXPECT_EQ(leds[4], fuchsia_input_report::wire::LedType::kKana);
}

TEST_F(ControllerTest, KeyboardPressTest) {
  InitDevices();
  zx_device* dev = controller_dev_->GetLatestChild();
  auto keyboard = dev->GetDeviceContext<i8042::I8042Device>();

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader;
  {
    fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
        fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
    fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
        client_->GetInputReportsReaderV2(std::move(endpoints.server), 10);
    ASSERT_OK(result.status());
    reader = fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>(
        std::move(endpoints.client));
    ASSERT_OK(keyboard->WaitForNextReader(zx::duration::infinite()));
  }
  {
    i8042_.SendDataAndIrq(false, 0x2);

    bool got_report = false;
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
          ASSERT_EQ(1u, reports.size());

          const fuchsia_input_report::wire::InputReport& report = reports[0];
          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_keyboard());
          const fuchsia_input_report::wire::KeyboardInputReport& keyboard_report =
              report.keyboard();

          ASSERT_TRUE(keyboard_report.has_pressed_keys3());
          ASSERT_EQ(keyboard_report.pressed_keys3().size(), 1u);
          EXPECT_EQ(keyboard_report.pressed_keys3()[0], fuchsia_input::wire::Key::kKey1);
          got_report = true;
        });
    ASSERT_OK(status);
    EXPECT_TRUE(got_report);
  }
  {
    i8042_.SendDataAndIrq(false, i8042::kKeyUp | 0x2);

    bool got_report = false;
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
          ASSERT_EQ(1u, reports.size());

          const fuchsia_input_report::wire::InputReport& report = reports[0];
          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_keyboard());
          const fuchsia_input_report::wire::KeyboardInputReport& keyboard_report =
              report.keyboard();

          ASSERT_TRUE(keyboard_report.has_pressed_keys3());
          EXPECT_EQ(keyboard_report.pressed_keys3().size(), 0u);
          got_report = true;
        });
    ASSERT_OK(status);
    EXPECT_TRUE(got_report);
  }
}

TEST_F(ControllerTest, GetMouseDescriptorTest) {
  i8042_.EnablePort2();
  InitDevices();

  auto response = client_->GetDescriptor();

  ASSERT_TRUE(response.ok());
  ASSERT_TRUE(response.value().descriptor.has_device_information());
  EXPECT_EQ(response.value().descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(response.value().descriptor.device_information().product_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kPcPs2Mouse));

  ASSERT_TRUE(response.value().descriptor.has_mouse());
  ASSERT_TRUE(response.value().descriptor.mouse().has_input());
  ASSERT_TRUE(response.value().descriptor.mouse().input().has_buttons());
  ASSERT_EQ(response.value().descriptor.mouse().input().buttons().size(), 3);
  EXPECT_EQ(response.value().descriptor.mouse().input().buttons()[0], 0x01);
  EXPECT_EQ(response.value().descriptor.mouse().input().buttons()[1], 0x02);
  EXPECT_EQ(response.value().descriptor.mouse().input().buttons()[2], 0x03);

  ASSERT_TRUE(response.value().descriptor.mouse().input().has_movement_x());
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_x().range.min, -127);
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_x().range.max, 127);
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_x().unit.type,
            fuchsia_input_report::wire::UnitType::kNone);
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_x().unit.exponent, 0);

  ASSERT_TRUE(response.value().descriptor.mouse().input().has_movement_y());
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_y().range.min, -127);
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_y().range.max, 127);
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_y().unit.type,
            fuchsia_input_report::wire::UnitType::kNone);
  EXPECT_EQ(response.value().descriptor.mouse().input().movement_y().unit.exponent, 0);
}

TEST_F(ControllerTest, MouseMoveTest) {
  i8042_.EnablePort2();
  InitDevices();
  zx_device* dev = controller_dev_->GetLatestChild();
  auto mouse = dev->GetDeviceContext<i8042::I8042Device>();

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader;
  {
    fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
        fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
    fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
        client_->GetInputReportsReaderV2(std::move(endpoints.server), 10);
    ASSERT_OK(result.status());
    reader = fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>(
        std::move(endpoints.client));
    ASSERT_OK(mouse->WaitForNextReader(zx::duration::infinite()));
  }

  i8042_.SendData(0x09 /* button_left | always_one */);
  i8042_.SendData(0x70) /* rel_x */;
  i8042_.SendDataAndIrq(true, 0x10 /* rel_y */);

  bool got_report = false;
  zx_status_t status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
        ASSERT_EQ(1u, reports.size());

        const fuchsia_input_report::wire::InputReport& report = reports[0];
        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse_report = report.mouse();

        ASSERT_TRUE(mouse_report.has_pressed_buttons());
        ASSERT_EQ(mouse_report.pressed_buttons().size(), 1u);
        EXPECT_EQ(mouse_report.pressed_buttons()[0], 0x1);
        ASSERT_TRUE(mouse_report.has_movement_x());
        EXPECT_EQ(mouse_report.movement_x(), 0x70);
        ASSERT_TRUE(mouse_report.has_movement_y());
        EXPECT_EQ(mouse_report.movement_y(), -16);
        got_report = true;
      });
  ASSERT_OK(status);
  EXPECT_TRUE(got_report);
}
