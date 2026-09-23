// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/input/drivers/hid/hid.h"

#include <fidl/fuchsia.hardware.hidbus/cpp/wire_test_base.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/hid/ambient-light.h>
#include <lib/hid/boot.h>
#include <lib/hid/paradise.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace hid_driver {

namespace fhidbus = fuchsia_hardware_hidbus;
namespace finput = fuchsia_hardware_input;

class FakeHidbus : public fidl::testing::WireTestBase<fhidbus::Hidbus> {
 public:
  fhidbus::Service::InstanceHandler GetInstanceHandler() {
    return fhidbus::Service::InstanceHandler({
        .device =
            [this](fidl::ServerEnd<fhidbus::Hidbus> server) {
              binding_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                  std::move(server), this, fidl::kIgnoreBindingClosure);
            },
    });
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ASSERT_TRUE(false);
  }

  const fhidbus::HidInfo& Query() { return info_; }
  void Query(QueryCompleter::Sync& completer) override {
    fidl::Arena<> arena;
    completer.ReplySuccess(fidl::ToWire(arena, info_));
  }
  void Start(StartCompleter::Sync& completer) override {
    if (start_status_ != ZX_OK) {
      completer.ReplyError(start_status_);
      return;
    }
    completer.ReplySuccess();
  }
  void Stop(StopCompleter::Sync& completer) override {}
  void SetDescriptor(fhidbus::wire::HidbusSetDescriptorRequest* request,
                     SetDescriptorCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetDescriptor(fhidbus::wire::HidbusGetDescriptorRequest* request,
                     GetDescriptorCompleter::Sync& completer) override {
    completer.ReplySuccess(
        fidl::VectorView<uint8_t>::FromExternal(report_desc_.data(), report_desc_.size()));
  }
  void GetReport(fhidbus::wire::HidbusGetReportRequest* request,
                 GetReportCompleter::Sync& completer) override {
    if (request->rpt_id != last_set_report_id_) {
      completer.ReplyError(ZX_ERR_INTERNAL);
      return;
    }
    if (request->len < last_set_report_.size()) {
      completer.ReplyError(ZX_ERR_BUFFER_TOO_SMALL);
    }

    completer.ReplySuccess(
        fidl::VectorView<uint8_t>::FromExternal(last_set_report_.data(), last_set_report_.size()));
  }
  void SetReport(fhidbus::wire::HidbusSetReportRequest* request,
                 SetReportCompleter::Sync& completer) override {
    last_set_report_id_ = request->rpt_id;
    last_set_report_ =
        std::vector<uint8_t>(request->data.data(), request->data.data() + request->data.size());
    completer.ReplySuccess();
  }
  void GetIdle(fhidbus::wire::HidbusGetIdleRequest* request,
               GetIdleCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }
  void SetIdle(fhidbus::wire::HidbusSetIdleRequest* request,
               SetIdleCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void GetProtocol(GetProtocolCompleter::Sync& completer) override {
    completer.ReplySuccess(hid_protocol_);
  }
  void SetProtocol(fhidbus::wire::HidbusSetProtocolRequest* request,
                   SetProtocolCompleter::Sync& completer) override {
    hid_protocol_ = request->protocol;
    completer.ReplySuccess();
  }

  void SetProtocol(fhidbus::wire::HidProtocol proto) { hid_protocol_ = proto; }
  void SetHidInfo(fhidbus::wire::HidInfo info) { info_ = fidl::ToNatural(info); }
  void SetStartStatus(zx_status_t status) { start_status_ = status; }
  void SendReport(uint8_t* report_data, size_t report_size) {
    SendReportWithTime(report_data, report_size, zx_clock_get_monotonic());
  }
  void SendReportWithTime(uint8_t* report_data, size_t report_size, zx_time_t time) {
    binding_.ForEachBinding([&](const auto& binding) {
      fidl::Arena arena;
      auto result = fidl::WireSendEvent(binding)->OnReportReceived(
          fhidbus::wire::Report::Builder(arena)
              .buf(fidl::VectorView<uint8_t>::FromExternal(report_data, report_size))
              .timestamp(time)
              .Build());
      ASSERT_TRUE(result.ok());
    });
  }
  void SetDescriptor(const uint8_t* desc, size_t desc_len) {
    report_desc_ = std::vector<uint8_t>(desc, desc + desc_len);
  }

 protected:
  std::vector<uint8_t> report_desc_;

  std::vector<uint8_t> last_set_report_;
  uint8_t last_set_report_id_;

  fhidbus::wire::HidProtocol hid_protocol_ = fhidbus::wire::HidProtocol::kReport;
  fhidbus::HidInfo info_;
  zx_status_t start_status_ = ZX_OK;

 private:
  fidl::ServerBindingGroup<fhidbus::Hidbus> binding_;
};

class HidDriverTestEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    return to_driver_vfs.AddService<fhidbus::Service>(fake_hidbus_.GetInstanceHandler());
  }

  FakeHidbus& fake_hidbus() { return fake_hidbus_; }

 private:
  FakeHidbus fake_hidbus_;
};

class TestConfig final {
 public:
  using DriverType = hid_driver::HidDriver;
  using EnvironmentType = HidDriverTestEnvironment;
};

class HidDeviceTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void SetupBootMouseDevice() {
    driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
      size_t desc_size;
      const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&desc_size);
      env.fake_hidbus().SetDescriptor(boot_mouse_desc, desc_size);

      fidl::Arena<> arena;
      env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                       .boot_protocol(fhidbus::wire::HidBootProtocol::kPointer)
                                       .vendor_id(0xabc)
                                       .product_id(123)
                                       .version(5)
                                       .dev_num(0)
                                       .polling_rate(0)
                                       .Build());
    });
  }

  fidl::ClientEnd<finput::Device> GetClient() {
    auto endpoints = fidl::Endpoints<finput::Device>::Create();

    auto result = driver_test().driver()->hiddev().CreateInstance(std::move(endpoints.server));
    EXPECT_OK(result);

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result = fidl::WireCall(endpoints.client)->GetReportsEvent();
                      ASSERT_TRUE(result.ok());
                      ASSERT_TRUE(result->is_ok());
                      report_event_ = std::move(result.value()->event);
                    })
                    .is_ok());

    return std::move(endpoints.client);
  }

  void ReadOneReport(fidl::WireSyncClient<finput::Device>& client, uint8_t* report_data,
                     size_t report_size, size_t* returned_size) {
    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      ASSERT_OK(
                          report_event_.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));

                      auto result = client->ReadReport();
                      ASSERT_TRUE(result.ok());
                      ASSERT_TRUE(result->is_ok());
                      ASSERT_TRUE(result.value()->has_buf());
                      ASSERT_TRUE(result.value()->buf().size() <= report_size);

                      for (size_t i = 0; i < result.value()->buf().size(); i++) {
                        report_data[i] = result.value()->buf()[i];
                      }
                      *returned_size = result.value()->buf().size();
                    })
                    .is_ok());
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 protected:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
  zx::event report_event_;
};

TEST_F(HidDeviceTest, LifeTimeTest) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());
}

TEST_F(HidDeviceTest, TestQuery) {
  // Ids were chosen arbitrarily.
  constexpr uint16_t kVendorId = 0xacbd;
  constexpr uint16_t kProductId = 0xdcba;
  constexpr uint16_t kVersion = 0x1234;

  SetupBootMouseDevice();
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    fidl::Arena<> arena;
    env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                     .boot_protocol(fhidbus::wire::HidBootProtocol::kPointer)
                                     .vendor_id(kVendorId)
                                     .product_id(kProductId)
                                     .version(kVersion)
                                     .dev_num(0)
                                     .polling_rate(0)
                                     .Build());
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->Query();
                    ASSERT_OK(result.status());

                    ASSERT_EQ(kVendorId, result.value()->info.vendor_id());
                    ASSERT_EQ(kProductId, result.value()->info.product_id());
                    ASSERT_EQ(kVersion, result.value()->info.version());
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, BootMouseSendReport) {
  SetupBootMouseDevice();
  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });

  uint8_t returned_report[3] = {};
  size_t actual;
  ReadOneReport(client, returned_report, sizeof(returned_report), &actual);

  ASSERT_EQ(actual, sizeof(returned_report));
  for (size_t i = 0; i < actual; i++) {
    ASSERT_EQ(returned_report[i], mouse_report[i]);
  }
}

TEST_F(HidDeviceTest, BootMouseSendReportWithTime) {
  SetupBootMouseDevice();
  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  const zx_time_t kTimestamp = 0xabcd;
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReportWithTime(mouse_report, sizeof(mouse_report), kTimestamp);
  });
  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    ASSERT_OK(
                        report_event_.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));

                    auto result = client->ReadReport();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_TRUE(result.value()->has_timestamp());
                    ASSERT_EQ(result.value()->timestamp(), kTimestamp);
                  })
                  .is_ok());
  ASSERT_NO_FATAL_FAILURE();
}

TEST_F(HidDeviceTest, BootMouseSendReportInPieces) {
  SetupBootMouseDevice();
  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(&mouse_report[0], sizeof(uint8_t));
    env.fake_hidbus().SendReport(&mouse_report[1], sizeof(uint8_t));
    env.fake_hidbus().SendReport(&mouse_report[2], sizeof(uint8_t));
  });

  uint8_t returned_report[3] = {};
  size_t actual;
  ReadOneReport(client, returned_report, sizeof(returned_report), &actual);

  ASSERT_EQ(actual, sizeof(returned_report));
  for (size_t i = 0; i < actual; i++) {
    ASSERT_EQ(returned_report[i], mouse_report[i]);
  }
}

TEST_F(HidDeviceTest, BootMouseSendMultipleReports) {
  SetupBootMouseDevice();
  uint8_t double_mouse_report[] = {0xDE, 0xAD, 0xBE, 0x12, 0x34, 0x56};
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  driver_test().RunInEnvironmentTypeContext([&double_mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(double_mouse_report, sizeof(double_mouse_report));
  });

  uint8_t returned_report[3] = {};
  size_t actual;

  // Read the first report.
  ReadOneReport(client, returned_report, sizeof(returned_report), &actual);
  ASSERT_EQ(actual, sizeof(returned_report));
  for (size_t i = 0; i < actual; i++) {
    ASSERT_EQ(returned_report[i], double_mouse_report[i]);
  }

  // Read the second report.
  ReadOneReport(client, returned_report, sizeof(returned_report), &actual);
  ASSERT_EQ(actual, sizeof(returned_report));
  for (size_t i = 0; i < actual; i++) {
    ASSERT_EQ(returned_report[i], double_mouse_report[i + 3]);
  }
}

TEST_F(HidDeviceTest, FailToRegister) {
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    fidl::Arena<> arena;
    env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                     .boot_protocol(fhidbus::wire::HidBootProtocol::kOther)
                                     .vendor_id(0)
                                     .product_id(0)
                                     .version(0)
                                     .dev_num(0)
                                     .polling_rate(0)
                                     .Build());
    env.fake_hidbus().SetStartStatus(ZX_ERR_INTERNAL);
  });
  ASSERT_EQ(driver_test().StartDriver().status_value(), ZX_ERR_INTERNAL);
}

TEST_F(HidDeviceTest, ReadReportSingleReport) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  // Send the reports.
  const zx_time_t time = 0xabcd;
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReportWithTime(mouse_report, sizeof(mouse_report), time);
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReport();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_EQ(time, result.value()->timestamp());
                    ASSERT_EQ(sizeof(mouse_report), result.value()->buf().size());
                    for (size_t i = 0; i < result.value()->buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result.value()->buf()[i]);
                    }
                  })
                  .is_ok());

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReport();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_error());
                    ASSERT_EQ(result->error_value(), ZX_ERR_SHOULD_WAIT);
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, ReadReportDoubleReport) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t double_mouse_report[] = {0xDE, 0xAD, 0xBE, 0x12, 0x34, 0x56};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  // Send the reports.
  const zx_time_t time = 0xabcd;
  driver_test().RunInEnvironmentTypeContext([&double_mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReportWithTime(double_mouse_report, sizeof(double_mouse_report), time);
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReport();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_EQ(time, result.value()->timestamp());
                    ASSERT_EQ(sizeof(hid_boot_mouse_report_t), result.value()->buf().size());
                    for (size_t i = 0; i < result.value()->buf().size(); i++) {
                      EXPECT_EQ(double_mouse_report[i], result.value()->buf()[i]);
                    }
                  })
                  .is_ok());

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReport();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_EQ(time, result.value()->timestamp());
                    ASSERT_EQ(sizeof(hid_boot_mouse_report_t), result.value()->buf().size());
                    for (size_t i = 0; i < result.value()->buf().size(); i++) {
                      EXPECT_EQ(double_mouse_report[i + sizeof(hid_boot_mouse_report_t)],
                                result.value()->buf()[i]);
                    }
                  })
                  .is_ok());

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReport();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_error());
                    ASSERT_EQ(result->error_value(), ZX_ERR_SHOULD_WAIT);
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, ReadReportsSingleReport) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  // Send the reports.
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReports();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_EQ(sizeof(mouse_report), result.value()->data.size());
                    for (size_t i = 0; i < result.value()->data.size(); i++) {
                      EXPECT_EQ(mouse_report[i], result.value()->data[i]);
                    }
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, ReadReportsDoubleReport) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t double_mouse_report[] = {0xDE, 0xAD, 0xBE, 0x12, 0x34, 0x56};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  // Send the reports.
  driver_test().RunInEnvironmentTypeContext([&double_mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(double_mouse_report, sizeof(double_mouse_report));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReports();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_EQ(sizeof(double_mouse_report), result.value()->data.size());
                    for (size_t i = 0; i < result.value()->data.size(); i++) {
                      EXPECT_EQ(double_mouse_report[i], result.value()->data[i]);
                    }
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, ReadReportsBlockingWait) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};

  auto dispatcher = driver_test().runtime().StartBackgroundDispatcher();
  ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&]() {
    sleep(1);
    driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
      env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
    });
  }));

  uint8_t returned_report[3] = {};
  size_t actual;
  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  ReadOneReport(client, returned_report, sizeof(returned_report), &actual);
  ASSERT_EQ(sizeof(mouse_report), actual);
  for (size_t i = 0; i < actual; i++) {
    EXPECT_EQ(mouse_report[i], returned_report[i]);
  }
}

// Test that only whole reports get sent through.
TEST_F(HidDeviceTest, ReadReportsOneAndAHalfReports) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  // Send the report.
  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });

  // Send a half of a report.
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    uint8_t half_report[] = {0xDE, 0xAD};
    env.fake_hidbus().SendReport(half_report, sizeof(half_report));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->ReadReports();
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                    ASSERT_EQ(sizeof(mouse_report), result.value()->data.size());
                    for (size_t i = 0; i < result.value()->data.size(); i++) {
                      EXPECT_EQ(mouse_report[i], result.value()->data[i]);
                    }
                  })
                  .is_ok());
}

// This tests that we can set the boot mode for a non-boot device, and that the device will
// have it's report descriptor set to the boot mode descriptor. For this, we will take an
// arbitrary descriptor and claim that it can be set to a boot-mode keyboard. We then
// test that the report descriptor we get back is for the boot keyboard.
// (The descriptor doesn't matter, as long as a device claims its a boot device it should
//  support this transformation in hardware).
TEST_F(HidDeviceTest, SettingBootModeMouse) {
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    size_t desc_size;
    const uint8_t* desc = get_paradise_touchpad_v1_report_desc(&desc_size);
    env.fake_hidbus().SetDescriptor(desc, desc_size);

    fidl::Arena<> arena;
    env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                     .boot_protocol(fhidbus::wire::HidBootProtocol::kPointer)
                                     .vendor_id(0)
                                     .product_id(0)
                                     .version(0)
                                     .dev_num(0)
                                     .polling_rate(0)
                                     .Build());

    // Set the device to boot protocol.
    env.fake_hidbus().SetProtocol(fhidbus::wire::HidProtocol::kBoot);
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  size_t boot_mouse_desc_size;
  const uint8_t* boot_mouse_desc = get_boot_mouse_report_desc(&boot_mouse_desc_size);
  ASSERT_EQ(boot_mouse_desc_size, driver_test().driver()->hiddev().GetReportDescLen());
  const uint8_t* received_desc = driver_test().driver()->hiddev().GetReportDesc();
  for (size_t i = 0; i < boot_mouse_desc_size; i++) {
    ASSERT_EQ(boot_mouse_desc[i], received_desc[i]);
  }
}

// This tests that we can set the boot mode for a non-boot device, and that the device will
// have it's report descriptor set to the boot mode descriptor. For this, we will take an
// arbitrary descriptor and claim that it can be set to a boot-mode keyboard. We then
// test that the report descriptor we get back is for the boot keyboard.
// (The descriptor doesn't matter, as long as a device claims its a boot device it should
//  support this transformation in hardware).
TEST_F(HidDeviceTest, SettingBootModeKbd) {
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    size_t desc_size;
    const uint8_t* desc = get_paradise_touchpad_v1_report_desc(&desc_size);
    env.fake_hidbus().SetDescriptor(desc, desc_size);

    fidl::Arena<> arena;
    env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                     .boot_protocol(fhidbus::wire::HidBootProtocol::kKbd)
                                     .vendor_id(0)
                                     .product_id(0)
                                     .version(0)
                                     .dev_num(0)
                                     .polling_rate(0)
                                     .Build());

    // Set the device to boot protocol.
    env.fake_hidbus().SetProtocol(fhidbus::wire::HidProtocol::kBoot);
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  size_t boot_kbd_desc_size;
  const uint8_t* boot_kbd_desc = get_boot_kbd_report_desc(&boot_kbd_desc_size);
  ASSERT_EQ(boot_kbd_desc_size, driver_test().driver()->hiddev().GetReportDescLen());
  const uint8_t* received_desc = driver_test().driver()->hiddev().GetReportDesc();
  for (size_t i = 0; i < boot_kbd_desc_size; i++) {
    ASSERT_EQ(boot_kbd_desc[i], received_desc[i]);
  }
}

TEST_F(HidDeviceTest, GetDescriptor) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  EXPECT_TRUE(
      driver_test()
          .RunOnBackgroundDispatcherSync([&]() {
            size_t known_size;
            const uint8_t* known_descriptor = get_boot_mouse_report_desc(&known_size);

            auto result = client->GetReportDesc();
            ASSERT_TRUE(result.ok());

            ASSERT_EQ(known_size, result->desc.size());
            ASSERT_EQ(std::vector(known_descriptor, known_descriptor + known_size),
                      std::vector(result->desc.data(), result->desc.data() + result->desc.size()));
          })
          .is_ok());
}

TEST_F(HidDeviceTest, GetSetReport) {
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    const uint8_t* desc;
    size_t desc_size = get_ambient_light_report_desc(&desc);
    env.fake_hidbus().SetDescriptor(desc, desc_size);

    fidl::Arena<> arena;
    env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                     .boot_protocol(fhidbus::wire::HidBootProtocol::kNone)
                                     .vendor_id(0)
                                     .product_id(0)
                                     .version(0)
                                     .dev_num(0)
                                     .polling_rate(0)
                                     .Build());
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  ambient_light_feature_rpt_t feature_report = {};
  feature_report.rpt_id = AMBIENT_LIGHT_RPT_ID_FEATURE;
  // Below value are chosen arbitrarily.
  feature_report.state = 100;
  feature_report.interval_ms = 50;
  feature_report.threshold_high = 40;
  feature_report.threshold_low = 10;

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->SetReport(
                        fhidbus::wire::ReportType::kFeature, AMBIENT_LIGHT_RPT_ID_FEATURE,
                        fidl::VectorView<uint8_t>::FromExternal(
                            reinterpret_cast<uint8_t*>(&feature_report), sizeof(feature_report)));
                    ASSERT_TRUE(result.ok());
                    ASSERT_TRUE(result->is_ok());
                  })
                  .is_ok());

  EXPECT_TRUE(
      driver_test()
          .RunOnBackgroundDispatcherSync([&]() {
            auto result = client->GetReport(fhidbus::wire::ReportType::kFeature,
                                            AMBIENT_LIGHT_RPT_ID_FEATURE);
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_ok());

            ASSERT_EQ(sizeof(feature_report), result->value()->report.size());
            ASSERT_EQ(
                std::vector(reinterpret_cast<uint8_t*>(&feature_report),
                            reinterpret_cast<uint8_t*>(&feature_report) + sizeof(feature_report)),
                std::vector(result->value()->report.data(),
                            result->value()->report.data() + result->value()->report.size()));
          })
          .is_ok());
}

// Tests that a device with too large reports don't cause buffer overruns.
TEST_F(HidDeviceTest, GetReportBufferOverrun) {
  driver_test().RunInEnvironmentTypeContext([](HidDriverTestEnvironment& env) {
    const uint8_t desc[] = {
        0x05, 0x01,                    // Usage Page (Generic Desktop Ctrls)
        0x09, 0x02,                    // Usage (Mouse)
        0xA1, 0x01,                    // Collection (Application)
        0x05, 0x09,                    //   Usage Page (Button)
        0x09, 0x30,                    //   Usage (0x30)
        0x97, 0x00, 0xF0, 0x00, 0x00,  //   Report Count (65279)
        0x75, 0x08,                    //   Report Size (8)
        0x25, 0x01,                    //   Logical Maximum (1)
        0x81, 0x02,  //   Input (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
        0xC0,        // End Collection

        // 22 bytes
    };
    size_t desc_size = sizeof(desc);
    env.fake_hidbus().SetDescriptor(desc, desc_size);

    fidl::Arena<> arena;
    env.fake_hidbus().SetHidInfo(fhidbus::wire::HidInfo::Builder(arena)
                                     .boot_protocol(fhidbus::wire::HidBootProtocol::kNone)
                                     .vendor_id(0)
                                     .product_id(0)
                                     .version(0)
                                     .dev_num(0)
                                     .polling_rate(0)
                                     .Build());
  });
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto result = client->GetReport(fhidbus::wire::ReportType::kInput, 0);
                    ASSERT_TRUE(result.ok());
                    ASSERT_FALSE(result->is_ok());
                    EXPECT_EQ(result->error_value(), ZX_ERR_INTERNAL);
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, DeviceReportReaderSingleReport) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  fidl::WireSyncClient<finput::DeviceReportsReader> reader;
  {
    auto endpoints = fidl::Endpoints<finput::DeviceReportsReader>::Create();

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result = client->GetDeviceReportsReader(std::move(endpoints.server));
                      ASSERT_OK(result.status());
                      reader = fidl::WireSyncClient<finput::DeviceReportsReader>(
                          std::move(endpoints.client));
                    })
                    .is_ok());
  }

  // Send the reports.
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto response = reader->ReadReports();
                    ASSERT_OK(response.status());
                    ASSERT_FALSE(response->is_error());
                    auto result = response->value();
                    ASSERT_EQ(result->reports.size(), 1UL);
                    ASSERT_EQ(result->reports[0].buf().size(), sizeof(mouse_report));
                    for (size_t i = 0; i < result->reports[0].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[0].buf()[i]);
                    }
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, DeviceReportReaderDoubleReport) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};
  uint8_t mouse_report_two[] = {0xDE, 0xAD, 0xBE};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  fidl::WireSyncClient<finput::DeviceReportsReader> reader;
  {
    auto endpoints = fidl::Endpoints<finput::DeviceReportsReader>::Create();

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result = client->GetDeviceReportsReader(std::move(endpoints.server));
                      ASSERT_OK(result.status());
                      reader = fidl::WireSyncClient<finput::DeviceReportsReader>(
                          std::move(endpoints.client));
                    })
                    .is_ok());
  }

  // Send the reports.
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });
  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    report_event_.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr);
                    report_event_.signal(ZX_USER_SIGNAL_0, 0);
                  })
                  .is_ok());
  driver_test().RunInEnvironmentTypeContext([&mouse_report_two](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report_two, sizeof(mouse_report_two));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    report_event_.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr);
                    auto response = reader->ReadReports();
                    ASSERT_OK(response.status());
                    ASSERT_FALSE(response->is_error());
                    auto result = response->value();
                    ASSERT_EQ(result->reports.size(), 2UL);
                    ASSERT_EQ(result->reports[0].buf().size(), sizeof(mouse_report));
                    for (size_t i = 0; i < result->reports[0].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[0].buf()[i]);
                    }
                    ASSERT_EQ(result->reports[1].buf().size(), sizeof(mouse_report));
                    for (size_t i = 0; i < result->reports[1].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[1].buf()[i]);
                    }
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, DeviceReportReaderTwoClients) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  fidl::WireSyncClient<finput::DeviceReportsReader> reader1;
  fidl::WireSyncClient<finput::DeviceReportsReader> reader2;
  {
    auto endpoints1 = fidl::Endpoints<finput::DeviceReportsReader>::Create();

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result1 = client->GetDeviceReportsReader(std::move(endpoints1.server));
                      ASSERT_OK(result1.status());
                      reader1 = fidl::WireSyncClient<finput::DeviceReportsReader>(
                          std::move(endpoints1.client));
                    })
                    .is_ok());

    auto endpoints2 = fidl::Endpoints<finput::DeviceReportsReader>::Create();

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result2 = client->GetDeviceReportsReader(std::move(endpoints2.server));
                      ASSERT_OK(result2.status());
                      reader2 = fidl::WireSyncClient<finput::DeviceReportsReader>(
                          std::move(endpoints2.client));
                    })
                    .is_ok());
  }

  // Send the report.
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto response = reader1->ReadReports();
                    ASSERT_OK(response.status());
                    ASSERT_FALSE(response->is_error());
                    auto result = response->value();
                    ASSERT_EQ(result->reports.size(), 1UL);
                    ASSERT_EQ(result->reports[0].buf().size(), sizeof(mouse_report));
                    for (size_t i = 0; i < result->reports[0].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[0].buf()[i]);
                    }
                  })
                  .is_ok());

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto response = reader2->ReadReports();
                    ASSERT_OK(response.status());
                    ASSERT_FALSE(response->is_error());
                    auto result = response->value();
                    ASSERT_EQ(result->reports.size(), 1UL);
                    ASSERT_EQ(result->reports[0].buf().size(), sizeof(mouse_report));
                    for (size_t i = 0; i < result->reports[0].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[0].buf()[i]);
                    }
                  })
                  .is_ok());
}

// Test that only whole reports get sent through.
TEST_F(HidDeviceTest, DeviceReportReaderOneAndAHalfReports) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  fidl::WireSyncClient<finput::DeviceReportsReader> reader;
  {
    auto endpoints = fidl::Endpoints<finput::DeviceReportsReader>::Create();

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result = client->GetDeviceReportsReader(std::move(endpoints.server));
                      ASSERT_OK(result.status());
                      reader = fidl::WireSyncClient<finput::DeviceReportsReader>(
                          std::move(endpoints.client));
                    })
                    .is_ok());
  }

  // Send the report.
  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};
  driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
  });

  // Send a half of a report.
  uint8_t half_report[] = {0xDE, 0xAD};
  driver_test().RunInEnvironmentTypeContext([&half_report](HidDriverTestEnvironment& env) {
    env.fake_hidbus().SendReport(half_report, sizeof(half_report));
  });

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto response = reader->ReadReports();
                    ASSERT_OK(response.status());
                    ASSERT_FALSE(response->is_error());
                    auto result = response->value();
                    ASSERT_EQ(result->reports.size(), 1UL);
                    ASSERT_EQ(sizeof(mouse_report), result->reports[0].buf().size());
                    for (size_t i = 0; i < result->reports[0].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[0].buf()[i]);
                    }
                  })
                  .is_ok());
}

TEST_F(HidDeviceTest, DeviceReportReaderHangingGet) {
  SetupBootMouseDevice();
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  uint8_t mouse_report[] = {0xDE, 0xAD, 0xBE};

  auto client = fidl::WireSyncClient<finput::Device>(GetClient());
  fidl::WireSyncClient<finput::DeviceReportsReader> reader;
  {
    auto endpoints = fidl::Endpoints<finput::DeviceReportsReader>::Create();

    EXPECT_TRUE(driver_test()
                    .RunOnBackgroundDispatcherSync([&]() {
                      auto result = client->GetDeviceReportsReader(std::move(endpoints.server));
                      ASSERT_OK(result.status());
                      reader = fidl::WireSyncClient<finput::DeviceReportsReader>(
                          std::move(endpoints.client));
                    })
                    .is_ok());
  }

  auto dispatcher = driver_test().runtime().StartBackgroundDispatcher();
  ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&]() {
    sleep(1);
    driver_test().RunInEnvironmentTypeContext([&mouse_report](HidDriverTestEnvironment& env) {
      env.fake_hidbus().SendReport(mouse_report, sizeof(mouse_report));
    });
  }));

  EXPECT_TRUE(driver_test()
                  .RunOnBackgroundDispatcherSync([&]() {
                    auto response = reader->ReadReports();
                    ASSERT_OK(response.status());
                    ASSERT_FALSE(response->is_error());
                    auto result = response->value();
                    ASSERT_EQ(result->reports.size(), 1UL);
                    ASSERT_EQ(sizeof(mouse_report), result->reports[0].buf().size());
                    for (size_t i = 0; i < result->reports[0].buf().size(); i++) {
                      EXPECT_EQ(mouse_report[i], result->reports[0].buf()[i]);
                    }
                  })
                  .is_ok());
}

}  // namespace hid_driver
