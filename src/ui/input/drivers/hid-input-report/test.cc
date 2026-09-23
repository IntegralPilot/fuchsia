// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.input/cpp/wire_test_base.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fit/function.h>
#include <lib/hid/acer12.h>
#include <lib/hid/ambient-light.h>
#include <lib/hid/boot.h>
#include <lib/hid/buttons.h>
#include <lib/hid/gt92xx.h>
#include <lib/hid/paradise.h>
#include <lib/hid/usages.h>
#include <zircon/assert.h>

#include <gtest/gtest.h>
#include <sdk/lib/inspect/testing/cpp/inspect.h>
#include <src/lib/testing/predicates/status.h>

#include "src/ui/input/drivers/hid-input-report/driver.h"

namespace hid_input_report_dev {

namespace fhidbus = fuchsia_hardware_hidbus;
namespace finput = fuchsia_hardware_input;

class SyncReaderV2EventHandler
    : public fidl::WireSyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
 public:
  using Callback = fit::function<void(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>;

  explicit SyncReaderV2EventHandler(Callback callback = nullptr) : callback_(std::move(callback)) {}

  void OnInputReports(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) override {
    last_report_stamp = event->last_report_stamp;
    if (callback_) {
      callback_(event);
    }
  }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

  uint64_t last_report_stamp = 0;

 private:
  Callback callback_;
};

class TestReaderV2EventHandler
    : public fidl::WireAsyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
 public:
  explicit TestReaderV2EventHandler(async::Loop& loop, size_t quit_after_reports = 1)
      : loop_(loop), quit_after_reports_(quit_after_reports) {}

  void OnInputReports(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) override {
    reports_received_ += event->reports.size();
    last_stamp_ = event->last_report_stamp;
    events_received_++;
    if (reports_received_ >= quit_after_reports_) {
      loop_.Quit();
    }
  }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

  size_t events_received() const { return events_received_; }
  size_t reports_received() const { return reports_received_; }
  uint64_t last_stamp() const { return last_stamp_; }

  void set_quit_after_reports(size_t count) { quit_after_reports_ = count; }

 private:
  async::Loop& loop_;
  size_t quit_after_reports_;
  size_t events_received_ = 0;
  size_t reports_received_ = 0;
  uint64_t last_stamp_ = 0;
};

namespace {

template <typename T>
std::vector<uint8_t> ToBinaryVector(T* data, size_t size) {
  const uint8_t* begin = reinterpret_cast<const uint8_t*>(data);
  return std::vector(begin, begin + size);
}

template <typename T, typename = std::enable_if_t<!std::is_pointer_v<T>>>
std::vector<uint8_t> ToBinaryVector(const T& t) {
  const uint8_t* begin = reinterpret_cast<const uint8_t*>(&t);
  return ToBinaryVector(begin, sizeof(T));
}

}  // namespace

class FakeHidDevice : public fidl::WireServer<finput::Controller>,
                      public fidl::testing::WireTestBase<finput::Device> {
 public:
  class DeviceReportsReader : public fidl::WireServer<finput::DeviceReportsReader> {
   public:
    explicit DeviceReportsReader(fidl::ServerEnd<finput::DeviceReportsReader> server,
                                 libsync::Completion& unbound)
        : binding_(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server), this,
                   [&unbound](fidl::UnbindInfo) { unbound.Signal(); }) {}

    ~DeviceReportsReader() override { binding_.Close(ZX_ERR_PEER_CLOSED); }

    void ReadReports(ReadReportsCompleter::Sync& completer) override {
      ASSERT_FALSE(waiting_read_.has_value());
      waiting_read_.emplace(completer.ToAsync());
      wait_for_read_.Signal();
    }

    void SendReport(std::vector<uint8_t> report, zx::time timestamp) {
      fidl::Arena arena;
      std::vector<fhidbus::wire::Report> reports = {
          fhidbus::wire::Report::Builder(arena)
              .timestamp(timestamp.get())
              .buf(fidl::VectorView<uint8_t>::FromExternal(report.data(), report.size()))
              .Build()};
      waiting_read_->ReplySuccess(
          fidl::VectorView<fhidbus::wire::Report>::FromExternal(reports.data(), reports.size()));
      waiting_read_.reset();
    }

    libsync::Completion wait_for_read_;

   private:
    fidl::ServerBinding<finput::DeviceReportsReader> binding_;
    std::optional<ReadReportsCompleter::Async> waiting_read_;
  };

  static constexpr uint32_t kVendorId = 0xabc;
  static constexpr uint32_t kProductId = 123;
  static constexpr uint32_t kVersion = 5;

  zx_status_t Serve(fdf::OutgoingDirectory& to_driver_vfs) {
    finput::Service::InstanceHandler instance_handler({
        .controller =
            [this](fidl::ServerEnd<finput::Controller> server) {
              EXPECT_FALSE(binding_);
              binding_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server),
                               this, fidl::kIgnoreBindingClosure);
            },
    });
    return to_driver_vfs.AddService<finput::Service>(std::move(instance_handler)).status_value();
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ASSERT_TRUE(false);
  }

  void Query(QueryCompleter::Sync& completer) override {
    fidl::Arena arena;
    completer.ReplySuccess(fhidbus::wire::HidInfo::Builder(arena)
                               .vendor_id(kVendorId)
                               .product_id(kProductId)
                               .version(kVersion)
                               .Build());
  }

  void OpenSession(OpenSessionRequestView request, OpenSessionCompleter::Sync& completer) override {
    EXPECT_FALSE(device_binding_);
    device_binding_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                            std::move(request->session), this, fidl::kIgnoreBindingClosure);
  }

  void GetDeviceReportsReader(GetDeviceReportsReaderRequestView request,
                              GetDeviceReportsReaderCompleter::Sync& completer) override {
    ASSERT_FALSE(reader_);
    reader_ = std::make_unique<DeviceReportsReader>(std::move(request->reader), unbound_);
    completer.ReplySuccess();
  }

  void GetReportDesc(GetReportDescCompleter::Sync& completer) override {
    completer.Reply(
        fidl::VectorView<uint8_t>::FromExternal(report_desc_.data(), report_desc_.size()));
  }

  void GetReport(GetReportRequestView request, GetReportCompleter::Sync& completer) override {
    // If the client is Getting a report with a specific ID, check that it matches
    // our saved report.
    if ((request->id != 0) && (report_.size() > 0)) {
      if (request->id != report_[0]) {
        completer.ReplyError(ZX_ERR_WRONG_TYPE);
        return;
      }
    }

    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(report_.data(), report_.size()));
  }
  std::vector<uint8_t>& GetReport() { return report_; }

  void SetReport(SetReportRequestView request, SetReportCompleter::Sync& completer) override {
    report_ = ToBinaryVector(request->report.data(), request->report.size());
    completer.ReplySuccess();
  }
  void SetReport(std::vector<uint8_t> report) { report_ = std::move(report); }

  void SetReportDesc(std::vector<uint8_t> report_desc) { report_desc_ = std::move(report_desc); }

  void SendReport(std::vector<uint8_t> report, zx::time timestamp) const {
    ASSERT_TRUE(reader_);
    reader_->SendReport(std::move(report), timestamp);
  }

  std::unique_ptr<DeviceReportsReader> reader_;
  libsync::Completion unbound_;

 private:
  std::optional<fidl::ServerBinding<finput::Controller>> binding_;
  std::optional<fidl::ServerBinding<finput::Device>> device_binding_;

  std::vector<uint8_t> report_desc_;
  std::vector<uint8_t> report_;
};

class InputReportTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    EXPECT_OK(fake_hid_.Serve(to_driver_vfs));
    return zx::ok();
  }

  FakeHidDevice& fake_hid() { return fake_hid_; }

 private:
  FakeHidDevice fake_hid_;
};

class FixtureConfig final {
 public:
  using DriverType = InputReportDriver;
  using EnvironmentType = InputReportTestEnvironment;
};

class HidDevTest : public ::testing::Test {
 public:
  static constexpr uint16_t kDefaultMaxUnacknowledgedReports = 10;

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fidl::WireSyncClient<fuchsia_input_report::InputDevice> GetSyncClient() {
    auto connect_result =
        driver_test().RunInNodeContext<zx::result<zx::channel>>([](fdf_testing::TestNode& node) {
          return node.children().at("InputReport").ConnectToDevice();
        });
    EXPECT_TRUE(connect_result.is_ok());
    return fidl::WireSyncClient(
        fidl::ClientEnd<fuchsia_input_report::InputDevice>(std::move(connect_result.value())));
  }

  zx_status_t ReadAndAcknowledgeOneEvent(
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

  fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> GetReaderV2(
      uint16_t max_unacknowledged_reports_limit) {
    return GetReaderV2(GetSyncClient(), max_unacknowledged_reports_limit);
  }
  fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> GetReaderV2(
      const fidl::WireSyncClient<fuchsia_input_report::InputDevice>& sync_client,
      uint16_t max_unacknowledged_reports_limit) {
    auto endpoints = fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
    auto result = sync_client->GetInputReportsReaderV2(std::move(endpoints.server),
                                                       max_unacknowledged_reports_limit);
    EXPECT_OK(result.status());
    sync_completion_t* next_reader_wait;
    driver_test().RunInDriverContext([&next_reader_wait](InputReportDriver& driver) {
      next_reader_wait = &driver.input_report_for_testing().next_reader_wait();
    });
    EXPECT_OK(sync_completion_wait(next_reader_wait, ZX_TIME_INFINITE));
    sync_completion_reset(next_reader_wait);
    return std::move(endpoints.client);
  }

  void SendReport(std::vector<uint8_t> report, zx::time timestamp = zx::time::infinite()) {
    if (timestamp == zx::time::infinite()) {
      timestamp = zx::clock::get_monotonic();
    }

    libsync::Completion* wait_for_read;
    driver_test().RunInEnvironmentTypeContext([&wait_for_read](InputReportTestEnvironment& env) {
      ASSERT_TRUE(env.fake_hid().reader_);
      wait_for_read = &env.fake_hid().reader_->wait_for_read_;
    });
    wait_for_read->Wait();
    wait_for_read->Reset();
    driver_test().RunInEnvironmentTypeContext(
        [&report, &timestamp](InputReportTestEnvironment& env) {
          ASSERT_TRUE(env.fake_hid().reader_);
          env.fake_hid().reader_->SendReport(std::move(report), timestamp);
        });
    wait_for_read->Wait();
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(HidDevTest, HidLifetimeTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());
}

class UnregisterHidDevTest : public ::testing::Test {
 public:
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(UnregisterHidDevTest, InputReportUnregisterTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  zx::result prepare_stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, prepare_stop_result.status_value());

  libsync::Completion* unbound;
  driver_test().RunInEnvironmentTypeContext(
      [&unbound](InputReportTestEnvironment& env) { unbound = &env.fake_hid().unbound_; });
  unbound->Wait();
}

TEST_F(HidDevTest, InputReportUnregisterTestBindFailed) {
  // We don't set a input report on `fake_hid` so the bind will fail.
  ASSERT_EQ(driver_test().StartDriver().status_value(), ZX_ERR_INTERNAL);

  // Make sure that the InputReport class is not registered to the HID device.
  driver_test().RunInEnvironmentTypeContext(
      [](InputReportTestEnvironment& env) { ASSERT_FALSE(env.fake_hid().reader_); });
}

TEST_F(HidDevTest, GetReportDescTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto sync_client = GetSyncClient();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetDescriptor> result =
      sync_client->GetDescriptor();
  ASSERT_OK(result.status());

  auto& desc = result->descriptor;
  ASSERT_TRUE(desc.has_mouse());
  ASSERT_TRUE(desc.mouse().has_input());

  fuchsia_input_report::wire::MouseInputDescriptor& mouse = desc.mouse().input();
  ASSERT_TRUE(mouse.has_movement_x());
  ASSERT_EQ(-127, mouse.movement_x().range.min);
  ASSERT_EQ(127, mouse.movement_x().range.max);

  ASSERT_TRUE(mouse.has_movement_y());
  ASSERT_EQ(-127, mouse.movement_y().range.min);
  ASSERT_EQ(127, mouse.movement_y().range.max);
}

TEST_F(HidDevTest, ReportDescInfoTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto sync_client = GetSyncClient();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetDescriptor> result =
      sync_client->GetDescriptor();
  ASSERT_OK(result.status());

  auto& desc = result.value().descriptor;
  ASSERT_TRUE(desc.has_device_information());
  ASSERT_EQ(desc.device_information().vendor_id(), FakeHidDevice::kVendorId);
  ASSERT_EQ(desc.device_information().product_id(), FakeHidDevice::kProductId);
  ASSERT_EQ(desc.device_information().version(), FakeHidDevice::kVersion);
}

TEST_F(HidDevTest, ReadInputReportsTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  // GetReaderV2() must be called before SendReport() because SendReport() only sends reports to
  // existing readers.
  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      GetReaderV2(kDefaultMaxUnacknowledgedReports));
  SendReport(std::vector<uint8_t>{0xFF, 0x50, 0x70});

  bool received = false;
  zx_status_t status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());

        const fuchsia_input_report::wire::InputReport& report = event->reports[0];
        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_mouse());
        const fuchsia_input_report::wire::MouseInputReport& mouse = report.mouse();

        ASSERT_TRUE(mouse.has_movement_x());
        ASSERT_EQ(0x50, mouse.movement_x());

        ASSERT_TRUE(mouse.has_movement_y());
        ASSERT_EQ(0x70, mouse.movement_y());

        ASSERT_TRUE(mouse.has_pressed_buttons());
        const fidl::VectorView<uint8_t>& pressed_buttons = mouse.pressed_buttons();
        for (size_t i = 0; i < pressed_buttons.size(); i++) {
          ASSERT_EQ(i + 1, pressed_buttons[i]);
        }
        received = true;
      });
  ASSERT_OK(status);
  ASSERT_TRUE(received);
}

TEST_F(HidDevTest, ReadInputReportsHangingGetTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> client_end =
      GetReaderV2(kDefaultMaxUnacknowledgedReports);
  TestReaderV2EventHandler event_handler(loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(client_end), loop.dispatcher(), &event_handler);

  // Send the report.
  SendReport(std::vector<uint8_t>{0xFF, 0x50, 0x70});

  loop.Run();
  ASSERT_EQ(1u, event_handler.events_received());
  ASSERT_EQ(1u, event_handler.reports_received());
}

TEST_F(HidDevTest, CloseReaderWithOutstandingRead) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t boot_mouse_desc_size;
    const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(boot_mouse_desc, boot_mouse_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> client_end =
      GetReaderV2(kDefaultMaxUnacknowledgedReports);
  TestReaderV2EventHandler event_handler(loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(client_end), loop.dispatcher(), &event_handler);

  // Unbind the reader.
  reader = {};
}

TEST_F(HidDevTest, SensorTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    const uint8_t* sensor_desc_ptr;
    size_t sensor_desc_size = get_ambient_light_report_desc(&sensor_desc_ptr);
    env.fake_hid().SetReportDesc(ToBinaryVector(sensor_desc_ptr, sensor_desc_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto sync_client = GetSyncClient();

  // Get the report descriptor.
  fidl::WireResult<fuchsia_input_report::InputDevice::GetDescriptor> result =
      sync_client->GetDescriptor();
  ASSERT_OK(result.status());
  fuchsia_input_report::wire::DeviceDescriptor& desc = result.value().descriptor;
  ASSERT_TRUE(desc.has_sensor());
  ASSERT_TRUE(desc.sensor().has_input());
  ASSERT_EQ(desc.sensor().input().size(), 1UL);
  fuchsia_input_report::wire::SensorInputDescriptor& sensor_desc = desc.sensor().input()[0];
  ASSERT_TRUE(sensor_desc.has_values());
  ASSERT_EQ(4UL, sensor_desc.values().size());

  ASSERT_EQ(sensor_desc.values()[0].type,
            fuchsia_input_report::wire::SensorType::kLightIlluminance);
  ASSERT_EQ(sensor_desc.values()[0].axis.unit.type, fuchsia_input_report::wire::UnitType::kNone);

  ASSERT_EQ(sensor_desc.values()[1].type, fuchsia_input_report::wire::SensorType::kLightRed);
  ASSERT_EQ(sensor_desc.values()[1].axis.unit.type, fuchsia_input_report::wire::UnitType::kNone);

  ASSERT_EQ(sensor_desc.values()[2].type, fuchsia_input_report::wire::SensorType::kLightBlue);
  ASSERT_EQ(sensor_desc.values()[2].axis.unit.type, fuchsia_input_report::wire::UnitType::kNone);

  ASSERT_EQ(sensor_desc.values()[3].type, fuchsia_input_report::wire::SensorType::kLightGreen);
  ASSERT_EQ(sensor_desc.values()[3].axis.unit.type, fuchsia_input_report::wire::UnitType::kNone);

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      GetReaderV2(sync_client, kDefaultMaxUnacknowledgedReports));

  // Create the report.
  ambient_light_input_rpt_t report_data = {};
  // Values are arbitrarily chosen.
  const int kIlluminanceTestVal = 10;
  const int kRedTestVal = 101;
  const int kBlueTestVal = 5;
  const int kGreenTestVal = 3;
  report_data.rpt_id = AMBIENT_LIGHT_RPT_ID_INPUT;
  report_data.illuminance = kIlluminanceTestVal;
  report_data.red = kRedTestVal;
  report_data.blue = kBlueTestVal;
  report_data.green = kGreenTestVal;

  SendReport(ToBinaryVector(report_data));

  // Get the report.
  zx_status_t status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
        ASSERT_EQ(1UL, reports.size());

        ASSERT_TRUE(reports[0].has_sensor());
        const fuchsia_input_report::wire::SensorInputReport& sensor_report = reports[0].sensor();
        EXPECT_TRUE(sensor_report.has_values());
        EXPECT_EQ(4UL, sensor_report.values().size());

        // Check the report.
        // These will always match the ordering in the descriptor.
        EXPECT_EQ(kIlluminanceTestVal, sensor_report.values()[0]);
        EXPECT_EQ(kRedTestVal, sensor_report.values()[1]);
        EXPECT_EQ(kBlueTestVal, sensor_report.values()[2]);
        EXPECT_EQ(kGreenTestVal, sensor_report.values()[3]);
      });
  ASSERT_OK(status);
}

TEST_F(HidDevTest, GetTouchInputReportTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t desc_len;
    const uint8_t* report_desc = get_paradise_touch_report_desc(&desc_len);
    env.fake_hid().SetReportDesc(ToBinaryVector(report_desc, desc_len));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  // Get an InputReportsReaderV2.
  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      GetReaderV2(kDefaultMaxUnacknowledgedReports));

  // Spoof send a report.
  paradise_touch_t touch_report = {};
  touch_report.rpt_id = PARADISE_RPT_ID_TOUCH;
  touch_report.contact_count = 1;
  touch_report.fingers[0].flags = 0xFF;
  touch_report.fingers[0].x = 100;
  touch_report.fingers[0].y = 200;
  touch_report.fingers[0].finger_id = 1;

  SendReport(ToBinaryVector(touch_report));

  // Get the report.
  zx_status_t touch_status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
        ASSERT_EQ(1UL, reports.size());

        const fuchsia_input_report::wire::InputReport& report = reports[0];
        const fuchsia_input_report::wire::TouchInputReport& touch = report.touch();
        ASSERT_TRUE(touch.has_contacts());
        ASSERT_EQ(1UL, touch.contacts().size());
        const fuchsia_input_report::wire::ContactInputReport& contact = touch.contacts()[0];

        ASSERT_TRUE(contact.has_position_x());
        ASSERT_EQ(2500, contact.position_x());

        ASSERT_TRUE(contact.has_position_y());
        ASSERT_EQ(5000, contact.position_y());
      });
  ASSERT_OK(touch_status);
}

TEST_F(HidDevTest, GetTouchPadDescTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t desc_len;
    const uint8_t* report_desc = get_paradise_touchpad_v1_report_desc(&desc_len);
    env.fake_hid().SetReportDesc(ToBinaryVector(report_desc, desc_len));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto sync_client = GetSyncClient();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetDescriptor> result =
      sync_client->GetDescriptor();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().descriptor.has_touch());
  ASSERT_TRUE(result.value().descriptor.touch().has_input());
  fuchsia_input_report::wire::TouchInputDescriptor& touch =
      result.value().descriptor.touch().input();

  ASSERT_EQ(fuchsia_input_report::wire::TouchType::kTouchpad, touch.touch_type());
}

TEST_F(HidDevTest, KeyboardTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t keyboard_descriptor_size;
    const uint8_t* keyboard_descriptor = get_boot_kbd_report_desc(&keyboard_descriptor_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(keyboard_descriptor, keyboard_descriptor_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  // Get an InputReportsReaderV2.
  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      GetReaderV2(kDefaultMaxUnacknowledgedReports));

  // Spoof send a report.
  hid_boot_kbd_report keyboard_report = {};
  keyboard_report.usage[0] = HID_USAGE_KEY_A;
  keyboard_report.usage[1] = HID_USAGE_KEY_UP;
  keyboard_report.usage[2] = HID_USAGE_KEY_B;

  SendReport(ToBinaryVector(keyboard_report));

  // Get the report.
  zx_status_t kbd_status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
        ASSERT_EQ(1UL, reports.size());

        const fuchsia_input_report::wire::InputReport& report = reports[0];
        const fuchsia_input_report::wire::KeyboardInputReport& keyboard = report.keyboard();
        ASSERT_EQ(3UL, keyboard.pressed_keys3().size());
        EXPECT_EQ(fuchsia_input::wire::Key::kA, keyboard.pressed_keys3()[0]);
        EXPECT_EQ(fuchsia_input::wire::Key::kUp, keyboard.pressed_keys3()[1]);
        EXPECT_EQ(fuchsia_input::wire::Key::kB, keyboard.pressed_keys3()[2]);
      });
  ASSERT_OK(kbd_status);
}

TEST_F(HidDevTest, KeyboardOutputReportTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t keyboard_descriptor_size;
    const uint8_t* keyboard_descriptor = get_boot_kbd_report_desc(&keyboard_descriptor_size);
    env.fake_hid().SetReportDesc(ToBinaryVector(keyboard_descriptor, keyboard_descriptor_size));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto sync_client = GetSyncClient();
  // Make an output report.
  fidl::Arena allocator;
  fidl::VectorView<fuchsia_input_report::wire::LedType> led_view(allocator, 2);
  led_view[0] = fuchsia_input_report::wire::LedType::kNumLock;
  led_view[1] = fuchsia_input_report::wire::LedType::kScrollLock;
  fuchsia_input_report::wire::KeyboardOutputReport fidl_keyboard(allocator);
  fidl_keyboard.set_enabled_leds(allocator, std::move(led_view));

  fuchsia_input_report::wire::OutputReport output_report(allocator);
  output_report.set_keyboard(allocator, std::move(fidl_keyboard));
  // Send the report.
  fidl::WireResult<fuchsia_input_report::InputDevice::SendOutputReport> response =
      sync_client->SendOutputReport(std::move(output_report));
  ASSERT_TRUE(response.ok());
  ASSERT_TRUE(response->is_ok());
  // Check the hid output report.
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    EXPECT_EQ(env.fake_hid().GetReport(), std::vector<uint8_t>{0b101});
  });
}

TEST_F(HidDevTest, ConsumerControlTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    const uint8_t* descriptor;
    size_t descriptor_size = get_buttons_report_desc(&descriptor);
    env.fake_hid().SetReportDesc(ToBinaryVector(descriptor, descriptor_size));

    // Create the initial report that will be queried on OpenSession.
    struct buttons_input_rpt report = {};
    report.rpt_id = BUTTONS_RPT_ID_INPUT;
    env.fake_hid().SetReport(ToBinaryVector(report));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto sync_client = GetSyncClient();

  // Get the report descriptor.
  fidl::WireResult<fuchsia_input_report::InputDevice::GetDescriptor> result =
      sync_client->GetDescriptor();
  ASSERT_OK(result.status());
  fuchsia_input_report::wire::DeviceDescriptor& desc = result.value().descriptor;
  ASSERT_TRUE(desc.has_consumer_control());
  ASSERT_TRUE(desc.consumer_control().has_input());
  fuchsia_input_report::wire::ConsumerControlInputDescriptor& consumer_control_desc =
      desc.consumer_control().input();
  ASSERT_TRUE(consumer_control_desc.has_buttons());
  ASSERT_EQ(5UL, consumer_control_desc.buttons().size());

  ASSERT_EQ(consumer_control_desc.buttons()[0],
            fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
  ASSERT_EQ(consumer_control_desc.buttons()[1],
            fuchsia_input_report::wire::ConsumerControlButton::kVolumeDown);
  ASSERT_EQ(consumer_control_desc.buttons()[2],
            fuchsia_input_report::wire::ConsumerControlButton::kFactoryReset);
  ASSERT_EQ(consumer_control_desc.buttons()[3],
            fuchsia_input_report::wire::ConsumerControlButton::kCameraDisable);
  ASSERT_EQ(consumer_control_desc.buttons()[4],
            fuchsia_input_report::wire::ConsumerControlButton::kMicMute);

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      GetReaderV2(sync_client, kDefaultMaxUnacknowledgedReports));

  // Check the initial report.
  zx_status_t init_status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());
        ASSERT_TRUE(event->reports[0].has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& report =
            event->reports[0].consumer_control();
        EXPECT_TRUE(report.has_pressed_buttons());
        EXPECT_EQ(0UL, report.pressed_buttons().size());
      });
  ASSERT_OK(init_status);

  // Create another report.
  struct buttons_input_rpt report = {};
  report.rpt_id = BUTTONS_RPT_ID_INPUT;
  fill_button_in_report(fuchsia_buttons::GpioButtonId::kVolumeUp, true, &report);
  fill_button_in_report(fuchsia_buttons::GpioButtonId::kFdr, true, &report);
  fill_button_in_report(fuchsia_buttons::GpioButtonId::kMicMute, true, &report);

  SendReport(ToBinaryVector(report));

  // Check the second report.
  zx_status_t report_status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());
        ASSERT_TRUE(event->reports[0].has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& report =
            event->reports[0].consumer_control();
        EXPECT_TRUE(report.has_pressed_buttons());
        EXPECT_EQ(3UL, report.pressed_buttons().size());

        EXPECT_EQ(report.pressed_buttons()[0],
                  fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
        EXPECT_EQ(report.pressed_buttons()[1],
                  fuchsia_input_report::wire::ConsumerControlButton::kFactoryReset);
        EXPECT_EQ(report.pressed_buttons()[2],
                  fuchsia_input_report::wire::ConsumerControlButton::kMicMute);
      });
  ASSERT_OK(report_status);
}

TEST_F(HidDevTest, ConsumerControlTwoClientsTest) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    const uint8_t* descriptor;
    size_t descriptor_size = get_buttons_report_desc(&descriptor);
    env.fake_hid().SetReportDesc(ToBinaryVector(descriptor, descriptor_size));

    // Create the initial report that will be queried on OpenSession.
    struct buttons_input_rpt report = {};
    report.rpt_id = BUTTONS_RPT_ID_INPUT;
    env.fake_hid().SetReport(ToBinaryVector(report));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  // Open the device.
  auto client = GetSyncClient();

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader1(
      GetReaderV2(client, kDefaultMaxUnacknowledgedReports));
  zx_status_t status1 = ReadAndAcknowledgeOneEvent(
      reader1,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());
        ASSERT_TRUE(event->reports[0].has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& report =
            event->reports[0].consumer_control();
        EXPECT_TRUE(report.has_pressed_buttons());
        EXPECT_EQ(0UL, report.pressed_buttons().size());
      });
  ASSERT_OK(status1);

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader2(
      GetReaderV2(client, kDefaultMaxUnacknowledgedReports));
  zx_status_t status2 = ReadAndAcknowledgeOneEvent(
      reader2,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());
        ASSERT_TRUE(event->reports[0].has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& report =
            event->reports[0].consumer_control();
        EXPECT_TRUE(report.has_pressed_buttons());
        EXPECT_EQ(0UL, report.pressed_buttons().size());
      });
  ASSERT_OK(status2);

  // Create another report.
  struct buttons_input_rpt report = {};
  report.rpt_id = BUTTONS_RPT_ID_INPUT;
  fill_button_in_report(fuchsia_buttons::GpioButtonId::kVolumeUp, true, &report);
  fill_button_in_report(fuchsia_buttons::GpioButtonId::kFdr, true, &report);
  fill_button_in_report(fuchsia_buttons::GpioButtonId::kMicMute, true, &report);

  SendReport(std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&report),
                                  reinterpret_cast<uint8_t*>(&report) + sizeof(report)));

  zx_status_t status3 = ReadAndAcknowledgeOneEvent(
      reader1,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());
        ASSERT_TRUE(event->reports[0].has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& report =
            event->reports[0].consumer_control();
        EXPECT_TRUE(report.has_pressed_buttons());
        EXPECT_EQ(3UL, report.pressed_buttons().size());

        EXPECT_EQ(report.pressed_buttons()[0],
                  fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
        EXPECT_EQ(report.pressed_buttons()[1],
                  fuchsia_input_report::wire::ConsumerControlButton::kFactoryReset);
        EXPECT_EQ(report.pressed_buttons()[2],
                  fuchsia_input_report::wire::ConsumerControlButton::kMicMute);
      });
  ASSERT_OK(status3);

  zx_status_t status4 = ReadAndAcknowledgeOneEvent(
      reader2,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        ASSERT_EQ(1UL, event->reports.size());
        ASSERT_TRUE(event->reports[0].has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& report =
            event->reports[0].consumer_control();
        EXPECT_TRUE(report.has_pressed_buttons());
        EXPECT_EQ(3UL, report.pressed_buttons().size());

        EXPECT_EQ(report.pressed_buttons()[0],
                  fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
        EXPECT_EQ(report.pressed_buttons()[1],
                  fuchsia_input_report::wire::ConsumerControlButton::kFactoryReset);
        EXPECT_EQ(report.pressed_buttons()[2],
                  fuchsia_input_report::wire::ConsumerControlButton::kMicMute);
      });
  ASSERT_OK(status4);
}

TEST_F(HidDevTest, TouchLatencyMeasurements) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    const uint8_t* report_desc;
    const size_t desc_len = get_gt92xx_report_desc(&report_desc);
    env.fake_hid().SetReportDesc(ToBinaryVector(report_desc, desc_len));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  using namespace inspect::testing;
  driver_test().RunInDriverContext([](InputReportDriver& driver) {
    auto hierarchy = inspect::ReadFromVmo(driver.input_report_for_testing().InspectVmo());
    EXPECT_TRUE(hierarchy.is_ok());
  });

  // Send five reports, and verify that the inspect stats make sense.
  gt92xx_touch_t report = {};

  // Add some additional computed latency to make the results more realistic.
  const zx::time timestamp = zx::clock::get_monotonic() - zx::msec(15);

  SendReport(ToBinaryVector(report), timestamp);
  SendReport(ToBinaryVector(report), timestamp);
  SendReport(ToBinaryVector(report), timestamp);
  SendReport(ToBinaryVector(report), timestamp);
  SendReport(ToBinaryVector(report), timestamp - zx::msec(5));

  driver_test().RunInDriverContext([](InputReportDriver& driver) {
    auto hierarchy = inspect::ReadFromVmo(driver.input_report_for_testing().InspectVmo());
    EXPECT_TRUE(hierarchy.is_ok());
    const inspect::Hierarchy* root = hierarchy.value().GetByPath({"hid-input-report-touch"});
    ASSERT_TRUE(root);

    const auto* latency_histogram =
        root->node().get_property<inspect::UintArrayValue>("latency_histogram_usecs");
    ASSERT_TRUE(latency_histogram);
    uint64_t latency_bucket_sum = 0;
    for (const inspect::UintArrayValue::HistogramBucket& bucket : latency_histogram->GetBuckets()) {
      latency_bucket_sum += bucket.count;
    }

    EXPECT_EQ(latency_bucket_sum, 5UL);

    const auto* average_latency =
        root->node().get_property<inspect::UintPropertyValue>("average_latency_usecs");
    ASSERT_TRUE(average_latency);

    const auto* max_latency =
        root->node().get_property<inspect::UintPropertyValue>("max_latency_usecs");
    ASSERT_TRUE(max_latency);

    EXPECT_GE(max_latency->value(), average_latency->value());
  });
}

TEST_F(HidDevTest, InspectDeviceTypes) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t desc_len;
    const uint8_t* report_desc = get_paradise_touch_report_desc(&desc_len);
    env.fake_hid().SetReportDesc(ToBinaryVector(report_desc, desc_len));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  driver_test().RunInDriverContext([](InputReportDriver& driver) {
    auto hierarchy = inspect::ReadFromVmo(driver.input_report_for_testing().InspectVmo());
    EXPECT_TRUE(hierarchy.is_ok());
    const inspect::Hierarchy* root =
        hierarchy.value().GetByPath({"hid-input-report-touch,touch,mouse"});
    ASSERT_TRUE(root);

    const auto* device_types =
        root->node().get_property<inspect::StringPropertyValue>("device_types");
    ASSERT_TRUE(device_types);

    EXPECT_STREQ(device_types->value().c_str(), "touch,touch,mouse");
  });
}

TEST_F(HidDevTest, ReaderV2) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t desc_len;
    const uint8_t* report_desc = get_boot_mouse_report_desc(&desc_len);
    env.fake_hid().SetReportDesc(ToBinaryVector(report_desc, desc_len));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> client_end =
      GetReaderV2(kDefaultMaxUnacknowledgedReports);
  TestReaderV2EventHandler event_handler(loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> client(
      std::move(client_end), loop.dispatcher(), &event_handler);

  hid_boot_mouse_report_t report = {};
  report.rel_x = 10;
  report.rel_y = 5;
  SendReport(ToBinaryVector(report));

  loop.Run();
  ASSERT_EQ(1u, event_handler.events_received());
  ASSERT_EQ(1u, event_handler.reports_received());
  ASSERT_EQ(1u, event_handler.last_stamp());
}

TEST_F(HidDevTest, ReaderV2UnacknowledgedLimit) {
  driver_test().RunInEnvironmentTypeContext([](InputReportTestEnvironment& env) {
    size_t desc_len;
    const uint8_t* report_desc = get_boot_mouse_report_desc(&desc_len);
    env.fake_hid().SetReportDesc(ToBinaryVector(report_desc, desc_len));
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> client_end =
      GetReaderV2(kDefaultMaxUnacknowledgedReports);
  TestReaderV2EventHandler event_handler(loop, 15);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> client(
      std::move(client_end), loop.dispatcher(), &event_handler);

  hid_boot_mouse_report_t report = {};
  report.rel_x = 10;
  report.rel_y = 5;

  // Send reports up to and beyond the max unacknowledged limit (default 10).
  for (size_t i = 0; i < 15; i++) {
    SendReport(ToBinaryVector(report));
  }

  // Verify that prior to acknowledgment, no more than 10 reports are delivered
  // and channel buffer contains no extra unacknowledged reports.
  loop.RunUntilIdle();
  EXPECT_EQ(event_handler.reports_received(), 10u);

  // Acknowledge reports and verify streaming resumes to receive remaining reports.
  (void)client->AcknowledgeReports(event_handler.last_stamp());
  loop.Run();
  EXPECT_EQ(event_handler.reports_received(), 15u);
}

}  // namespace hid_input_report_dev
