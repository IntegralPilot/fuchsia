// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.virtual.bus/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.virtualbustest/cpp/fidl.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h>

#include <fbl/string.h>
#include <gtest/gtest.h>
#include <usb/usb.h>

namespace usb_virtual_bus {
namespace {

namespace virtualbustest = fuchsia_hardware_usb_virtualbustest;

constexpr const char kManufacturer[] = "Google";
constexpr const char kProduct[] = "USB Virtual Bus Virtual Device";
constexpr const char kSerial[] = "ebfd5ad49d2a";

class VirtualBusTest : public testing::Test {
 public:
  void SetUp() override {
    auto bus = usb_virtual::BusLauncher::Create({
        fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service{
            {.name = fuchsia_hardware_usb_virtualbustest::BusTestService::Name}}),
        fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service{
            {.name = fuchsia_hardware_usb_virtualbustest::ExpectBusTestService::Name}}),
    });
    ASSERT_EQ(bus.status_value(), ZX_OK);
    bus_ = std::move(bus.value());
    InitUsbVirtualBus();

    bus_test_.Bind(Connect<virtualbustest::BusTestService::Device>());
    async_bus_test_.Bind(Connect<virtualbustest::BusTestService::Device>(),
                         fdf::Dispatcher::GetCurrent()->async_dispatcher());
    expect_bus_test_.Bind(Connect<virtualbustest::ExpectBusTestService::Device>(),
                          fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  void TearDown() override {
    ASSERT_EQ(bus_->ClearPeripheralDeviceFunctions(), ZX_OK);
    ASSERT_EQ(bus_->Disable(), ZX_OK);
  }

  template <typename ServiceMember>
  fidl::ClientEnd<typename ServiceMember::ProtocolType> Connect() {
    component::SyncServiceMemberWatcher<ServiceMember> watcher(bus_->GetExposedDir());
    zx::result result = watcher.GetNextInstance(false);
    if (result.is_error()) {
      ADD_FAILURE() << "Failed to find service instance: " << result.status_string();
      return {};
    }
    return std::move(result.value());
  }

 protected:
  void InitUsbVirtualBus();
  fdf_testing::DriverRuntime& runtime() { return runtime_; }

  fidl::SyncClient<virtualbustest::BusTest> bus_test_;
  fidl::Client<virtualbustest::BusTest> async_bus_test_;
  fidl::Client<virtualbustest::ExpectBusTest> expect_bus_test_;
  fdf_testing::DriverRuntime runtime_;
  std::optional<usb_virtual::BusLauncher> bus_;
};

void VirtualBusTest::InitUsbVirtualBus() {
  namespace usb_peripheral = fuchsia_hardware_usb_peripheral;
  using ConfigurationDescriptor =
      ::fidl::VectorView<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>;

  usb_peripheral::wire::DeviceDescriptor device_desc = {};
  device_desc.bcd_usb = 0x0200;
  device_desc.b_device_class = 0;
  device_desc.b_device_sub_class = 0;
  device_desc.b_device_protocol = 0;
  device_desc.b_max_packet_size0 = 64;
  device_desc.bcd_device = 0x0100;
  device_desc.b_num_configurations = 1;

  device_desc.manufacturer = fidl::StringView(kManufacturer);
  device_desc.product = fidl::StringView(kProduct);
  device_desc.serial = fidl::StringView(kSerial);

  device_desc.id_vendor = 0x18D1;
  device_desc.id_product = 2;

  usb_peripheral::wire::FunctionDescriptor usb_cdc_ecm_function_desc = {
      .interface_class = USB_CLASS_VENDOR,
      .interface_subclass = 0,
      .interface_protocol = 0,
  };

  std::vector<usb_peripheral::wire::FunctionDescriptor> function_descs;
  function_descs.push_back(usb_cdc_ecm_function_desc);
  std::vector<ConfigurationDescriptor> config_descs;
  config_descs.emplace_back(
      fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(function_descs));

  ASSERT_EQ(bus_->SetupPeripheralDevice(std::move(device_desc), std::move(config_descs)), ZX_OK);
}

TEST_F(VirtualBusTest, ControlOutTransfer) {
  static const size_t kExpectedDataSize = 19;

  bool expect_completed = false;
  expect_bus_test_->ExpectControl({false, {}}).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    if (result.is_ok()) {
      for (size_t i = 0; i < kExpectedDataSize; i++) {
        EXPECT_EQ(result->out_data()[i], static_cast<uint8_t>(i));
      }
    }
    expect_completed = true;
  });

  bool sync_completed = false;
  expect_bus_test_->Sync().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    sync_completed = true;
  });
  runtime().RunUntil([&]() { return sync_completed; });

  std::vector<uint8_t> data(kExpectedDataSize);
  for (size_t i = 0; i < data.size(); i++) {
    data[i] = static_cast<uint8_t>(i);
  }
  auto result = bus_test_->Control({false, std::move(data)});
  ASSERT_TRUE(result.is_ok());

  runtime().RunUntil([&]() { return expect_completed; });
  runtime().RunUntilIdle();
}

TEST_F(VirtualBusTest, ControlInTransfer) {
  static const size_t kExpectedDataSize = 3;

  std::vector<uint8_t> data(kExpectedDataSize);
  for (size_t i = 0; i < data.size(); i++) {
    data[i] = static_cast<uint8_t>(i);
  }
  bool expect_completed = false;
  expect_bus_test_->ExpectControl({true, std::move(data)}).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    expect_completed = true;
  });

  bool sync_completed = false;
  expect_bus_test_->Sync().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    sync_completed = true;
  });
  runtime().RunUntil([&]() { return sync_completed; });

  auto result = bus_test_->Control({true, {}});
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->in_data().size(), kExpectedDataSize);
  for (size_t i = 0; i < kExpectedDataSize; i++) {
    EXPECT_EQ(result->in_data()[i], static_cast<uint8_t>(i));
  }

  runtime().RunUntil([&]() { return expect_completed; });
  runtime().RunUntilIdle();
}

TEST_F(VirtualBusTest, OutTransfer) {
  static const size_t kExpectedDataSize = 8;

  bool expect_completed = false;
  expect_bus_test_->ExpectOut().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    if (result.is_ok()) {
      EXPECT_EQ(result->data().size(), kExpectedDataSize);
      for (size_t i = 0; i < result->data().size(); i++) {
        EXPECT_EQ(result->data()[i], static_cast<uint8_t>(i));
      }
    }
    expect_completed = true;
  });

  bool sync_completed = false;
  expect_bus_test_->Sync().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    sync_completed = true;
  });
  runtime().RunUntil([&]() { return sync_completed; });

  std::vector<uint8_t> data(kExpectedDataSize);
  for (size_t i = 0; i < data.size(); i++) {
    data[i] = static_cast<uint8_t>(i);
  }
  auto result = bus_test_->Out(std::move(data));
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->actual(), kExpectedDataSize);

  runtime().RunUntil([&]() { return expect_completed; });
  runtime().RunUntilIdle();
}

TEST_F(VirtualBusTest, InTransfer) {
  static const size_t kExpectedDataSize = 5;

  std::vector<uint8_t> data(kExpectedDataSize);
  for (size_t i = 0; i < data.size(); i++) {
    data[i] = static_cast<uint8_t>(i);
  }
  bool expect_completed = false;
  expect_bus_test_->ExpectIn(std::move(data)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    if (result.is_ok()) {
      EXPECT_EQ(result->actual(), kExpectedDataSize);
    }
    expect_completed = true;
  });

  bool sync_completed = false;
  expect_bus_test_->Sync().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    sync_completed = true;
  });
  runtime().RunUntil([&]() { return sync_completed; });

  auto result = bus_test_->In(kExpectedDataSize);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->data().size(), kExpectedDataSize);
  for (size_t i = 0; i < kExpectedDataSize; i++) {
    EXPECT_EQ(result->data()[i], static_cast<uint8_t>(i));
  }

  runtime().RunUntil([&]() { return expect_completed; });
  runtime().RunUntilIdle();
}

TEST_F(VirtualBusTest, Reconnect) {
  bool disconnected = false;
  expect_bus_test_->Connect(false).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    disconnected = true;
  });
  runtime().RunUntil([&]() { return disconnected; });

  bool connected = false;
  expect_bus_test_->Connect(true).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    connected = true;
  });
  runtime().RunUntil([&]() { return connected; });
}

TEST_F(VirtualBusTest, RepeatedConfiguration) {
  for (int i = 0; i < 10; ++i) {
    ASSERT_EQ(bus_->ClearPeripheralDeviceFunctions(), ZX_OK);
    ASSERT_EQ(bus_->Disconnect(), ZX_OK);

    ASSERT_NO_FATAL_FAILURE(InitUsbVirtualBus());
    ASSERT_EQ(bus_->Enable(), ZX_OK);

    bus_test_ = {};
    async_bus_test_ = {};
    expect_bus_test_ = {};

    bus_test_ = fidl::SyncClient<virtualbustest::BusTest>(
        Connect<virtualbustest::BusTestService::Device>());
    async_bus_test_.Bind(Connect<virtualbustest::BusTestService::Device>(),
                         fdf::Dispatcher::GetCurrent()->async_dispatcher());
    expect_bus_test_.Bind(Connect<virtualbustest::ExpectBusTestService::Device>(),
                          fdf::Dispatcher::GetCurrent()->async_dispatcher());

    // Verify transfers still work after re-configuration
    static const size_t kExpectedDataSize = 8;
    bool expect_completed = false;
    expect_bus_test_->ExpectOut().Then([&](auto& result) {
      EXPECT_TRUE(result.is_ok());
      if (result.is_ok()) {
        EXPECT_EQ(result->data().size(), kExpectedDataSize);
      }
      expect_completed = true;
    });

    bool sync_completed = false;
    expect_bus_test_->Sync().Then([&](auto& result) {
      EXPECT_TRUE(result.is_ok());
      sync_completed = true;
    });
    runtime().RunUntil([&]() { return sync_completed; });

    std::vector<uint8_t> data(kExpectedDataSize);
    for (size_t i = 0; i < data.size(); i++) {
      data[i] = static_cast<uint8_t>(i);
    }
    auto result = bus_test_->Out(std::move(data));
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->actual(), kExpectedDataSize);

    runtime().RunUntil([&]() { return expect_completed; });
    runtime().RunUntilIdle();
  }
}

TEST_F(VirtualBusTest, PendingRequestsDuringClearFunctions) {
  // Queue a request without setting expectation, so it remains pending.
  std::vector<uint8_t> data(8, 0xAA);
  bool request_completed = false;
  async_bus_test_->Out({data}).Then([&](auto& result) {
    // We expect this to fail or be cancelled because we are clearing functions.
    EXPECT_FALSE(result.is_ok());
    request_completed = true;
  });

  // Ensure the request is truly pending (not completed immediately).
  runtime().RunUntilIdle();
  ASSERT_FALSE(request_completed);

  // Trigger ClearFunctions using the launcher API. PerformBlockingWork runs the dispatcher
  // while waiting for the synchronous command to finish in the background.
  bool clear_completed = false;
  runtime().PerformBlockingWork([&]() {
    ASSERT_EQ(bus_->ClearPeripheralDeviceFunctions(), ZX_OK);
    clear_completed = true;
  });

  EXPECT_TRUE(request_completed);
  EXPECT_TRUE(clear_completed);
}

}  // namespace
}  // namespace usb_virtual_bus
