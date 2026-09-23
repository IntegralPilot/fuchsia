// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.fastboot/cpp/wire_test_base.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gtest/gtest.h>
#include <usb-inspect/usb-inspect-test-helper.h>

#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"
#include "src/firmware/drivers/usb-fastboot-function/usb_fastboot_function.h"

namespace usb_fastboot_function {
namespace {

constexpr uint32_t kBulkOutEp = 1;
constexpr uint32_t kBulkInEp = 2;

class FakeUsbFunction
    : public fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction> {
 public:
  using Base = fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction>;
  using Base::Base;

  void Configure(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure>& request,
      fidl::internal::NaturalCompleter<fuchsia_hardware_usb_function::UsbFunction::Configure>::Sync&
          completer) override {
    interface_ = std::move(request.iface());
    completer.Reply(fit::ok());
  }

  void AllocResources(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::AllocResources>::Sync& completer) override {
    fuchsia_hardware_usb_function::UsbFunctionAllocResourcesResponse response;
    ASSERT_EQ(request.endpoints().size(), 2u);
    ASSERT_EQ(request.interface_count(), 2u);
    response.interface_nums() = {0, 1};
    response.endpoint_addrs() = {kBulkOutEp, kBulkInEp};
    response.string_indices() = {};
    for (size_t i = 0; i < 2; i++) {
      fidl::ServerEnd ep = std::move(request.endpoints()[i].endpoint());
      fake_endpoint(response.endpoint_addrs()[i]).Connect(dispatcher(), std::move(ep));
    }
    completer.Reply(fit::ok(std::move(response)));
  }

  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> TakeInterface() {
    return std::move(interface_);
  }

 private:
  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> interface_;
};

class UsbFastbootEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    device_server_.Initialize("default", std::nullopt);
    EXPECT_EQ(ZX_OK, device_server_.Serve(dispatcher, &to_driver_vfs));
    fuchsia_hardware_usb_function::UsbFunctionService::InstanceHandler handler({
        .device = usb_function_bindings_.CreateHandler(&fake_dev_, dispatcher,
                                                       fidl::kIgnoreBindingClosure),
    });
    EXPECT_TRUE(
        to_driver_vfs
            .AddService<fuchsia_hardware_usb_function::UsbFunctionService>(std::move(handler))
            .is_ok());

    return zx::ok();
  }

  compat::DeviceServer device_server_;
  FakeUsbFunction fake_dev_ = FakeUsbFunction(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::ServerBindingGroup<fuchsia_hardware_usb_function::UsbFunction> usb_function_bindings_;
};

class UsbFastbootTestConfig final {
 public:
  using DriverType = UsbFastbootFunction;
  using EnvironmentType = UsbFastbootEnvironment;
};

class UsbFastbootFunctionTest : public ::testing::Test {
 protected:
  void SetUp() override {
    ASSERT_EQ(ZX_OK, driver_test_.StartDriver().status_value());
    auto device = driver_test_.Connect<fuchsia_hardware_fastboot::Service::Fastboot>();
    EXPECT_EQ(ZX_OK, device.status_value());
    client_.Bind(std::move(device.value()));

    driver_test_.RunInEnvironmentTypeContext([this](UsbFastbootEnvironment& env) {
      function_client_.Bind(env.fake_dev_.TakeInterface());
    });

    driver_test_.RunInDriverContext([](UsbFastbootFunction& driver) {
      EXPECT_EQ(driver.bulk_in_addr(), kBulkInEp);
      EXPECT_EQ(driver.bulk_out_addr(), kBulkOutEp);
    });
  }

  void TearDown() override {}

  void EnableUsb() {
    ASSERT_TRUE(function_client_.is_valid());
    {
      fidl::Result result = function_client_->SetConfigured({{
          .configured = true,
          .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
      }});
      ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
    }
    {
      fidl::Result result = function_client_->SetInterface({{
          .interface = 0,
          .alt_setting = 0,
      }});
      ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
    }
  }

  fidl::WireSyncClient<fuchsia_hardware_fastboot::FastbootImpl>& client() { return client_; }

 protected:
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunctionInterface> function_client_;
  fidl::WireSyncClient<fuchsia_hardware_fastboot::FastbootImpl> client_;
  fdf_testing::BackgroundDriverTest<UsbFastbootTestConfig> driver_test_;
};

TEST_F(UsbFastbootFunctionTest, LifetimeTest) {
  // Lifetime tested in test Setup() and TearDown()
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

void ValidateVmo(const zx::vmo& vmo, std::string_view payload) {
  fzl::VmoMapper mapper;
  ASSERT_EQ(ZX_OK, mapper.Map(vmo));
  size_t content_size = 0;
  ASSERT_EQ(ZX_OK, vmo.get_prop_content_size(&content_size));
  ASSERT_EQ(content_size, payload.size());
}

TEST_F(UsbFastbootFunctionTest, ReceiveTestSinglePacket) {
  const std::string_view test_data = "getvar:all";
  EnableUsb();

  std::thread t([&] {
    auto res = client()->Receive(0);
    ValidateVmo(res.value()->data, test_data);
  });

  driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
    env.fake_dev_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, test_data.size());
  });

  t.join();
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, ReceiveStateReset) {
  EnableUsb();

  {
    const std::string_view test_data = "getvar:all";
    std::thread t1([&] {
      auto res = client()->Receive(0);
      ValidateVmo(res.value()->data, test_data);
    });
    driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
      env.fake_dev_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, test_data.size());
    });
    t1.join();
  }

  {
    const std::string_view test_data = "getvar:max-download-size";
    std::thread t1([&] {
      auto res = client()->Receive(0);
      ValidateVmo(res.value()->data, test_data);
    });
    driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
      env.fake_dev_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, test_data.size());
    });
    t1.join();
  }
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, ReceiveFailsOnError) {
  const std::string_view test_data = "getvar:all";
  EnableUsb();

  std::thread t([&]() { ASSERT_FALSE(client()->Receive(0)->is_ok()); });

  driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
    env.fake_dev_.fake_endpoint(kBulkOutEp)
        .RequestComplete(ZX_ERR_IO_NOT_PRESENT, test_data.size());
  });

  t.join();
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

void InitializeSendVmo(fzl::OwnedVmoMapper& vmo, std::string_view data) {
  ASSERT_EQ(ZX_OK, vmo.CreateAndMap(data.size(), "test"));
  ASSERT_EQ(ZX_OK, vmo.vmo().set_prop_content_size(data.size()));
  memcpy(vmo.start(), data.data(), data.size());
}

TEST_F(UsbFastbootFunctionTest, ReceiveFailsOnNonConfiguredInterface) {
  ASSERT_FALSE(client()->Receive(0)->is_ok());
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, Send) {
  const std::string_view send_data = "OKAY0.4";
  fzl::OwnedVmoMapper send_vmo;
  InitializeSendVmo(send_vmo, send_data);

  EnableUsb();

  std::thread t([&]() {
    auto result = client()->Send(send_vmo.Release());
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  });

  driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
    env.fake_dev_.fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, send_data.size());
  });

  t.join();
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, SendStatesReset) {
  EnableUsb();

  {
    const std::string_view send_data = "OKAY0.4";
    fzl::OwnedVmoMapper send_vmo;
    InitializeSendVmo(send_vmo, send_data);
    std::thread t([&]() {
      auto result = client()->Send(send_vmo.Release());
      ASSERT_TRUE(result.ok());
      ASSERT_TRUE(result->is_ok());
    });
    driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
      env.fake_dev_.fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, send_data.size());
    });
    t.join();
  }

  {
    const std::string_view send_data = "OKAY0.6";
    fzl::OwnedVmoMapper send_vmo;
    InitializeSendVmo(send_vmo, send_data);
    std::thread t([&]() {
      auto result = client()->Send(send_vmo.Release());
      ASSERT_TRUE(result.ok());
      ASSERT_TRUE(result->is_ok());
    });
    driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
      env.fake_dev_.fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, send_data.size());
    });
    t.join();
  }
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, SendFailsOnNonConfiguredInterface) {
  const std::string_view send_data = "OKAY0.4";
  fzl::OwnedVmoMapper send_vmo;
  InitializeSendVmo(send_vmo, send_data);
  ASSERT_FALSE(client()->Send(send_vmo.Release())->is_ok());
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, SendFailOnError) {
  EnableUsb();

  const std::string_view send_data = "OKAY0.6";
  fzl::OwnedVmoMapper send_vmo;
  InitializeSendVmo(send_vmo, send_data);

  std::thread t([&]() { ASSERT_FALSE(client()->Send(send_vmo.Release())->is_ok()); });

  driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
    env.fake_dev_.fake_endpoint(kBulkInEp).RequestComplete(ZX_ERR_IO_NOT_PRESENT, send_data.size());
  });

  t.join();
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, SendFailsOnZeroContentSize) {
  EnableUsb();
  const std::string_view send_data = "OKAY0.4";
  fzl::OwnedVmoMapper send_vmo;
  InitializeSendVmo(send_vmo, send_data);
  ASSERT_EQ(ZX_OK, send_vmo.vmo().set_prop_content_size(0));
  ASSERT_FALSE(client()->Send(send_vmo.Release())->is_ok());
  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

TEST_F(UsbFastbootFunctionTest, Inspect) {
  EnableUsb();

  const std::string_view send_data = "OKAY0.4";
  const std::string_view recv_data = "getvar:all";

  // 1. Send (TX)
  fzl::OwnedVmoMapper send_vmo;
  InitializeSendVmo(send_vmo, send_data);
  std::thread t_send([&]() {
    auto result = client()->Send(send_vmo.Release());
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  });
  driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
    env.fake_dev_.fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, send_data.size());
  });
  t_send.join();

  // 2. Receive (RX)
  std::thread t_recv([&] {
    auto res = client()->Receive(0);
    ValidateVmo(res.value()->data, recv_data);
  });
  driver_test_.RunInEnvironmentTypeContext([&](UsbFastbootEnvironment& env) {
    env.fake_dev_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, recv_data.size());
  });
  t_recv.join();

  // 3. Trigger throughput and verify
  driver_test_.RunInDriverContext(
      [tx_size = send_data.size(), rx_size = recv_data.size()](UsbFastbootFunction& driver) {
        driver.GetThroughputTrackerForTesting().MeasureForTesting(zx::sec(1));

        auto hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());

        auto* fastboot_node = hierarchy.GetByPath({"usb-fastboot"});
        ASSERT_TRUE(fastboot_node != nullptr);

        auto* bulk_in = hierarchy.GetByPath({"usb-fastboot", "bulk_in"});
        ASSERT_TRUE(bulk_in != nullptr);
        auto err_in = usb_inspect::VerifyEndpointInspect(bulk_in, tx_size, std::nullopt, 0,
                                                         std::nullopt, tx_size);
        EXPECT_TRUE(err_in.is_ok()) << err_in.error_value();

        auto* bulk_out = hierarchy.GetByPath({"usb-fastboot", "bulk_out"});
        ASSERT_TRUE(bulk_out != nullptr);
        auto err_out = usb_inspect::VerifyEndpointInspect(bulk_out, std::nullopt, rx_size,
                                                          std::nullopt, 0, rx_size);
        EXPECT_TRUE(err_out.is_ok()) << err_out.error_value();
      });

  ASSERT_TRUE(driver_test_.StopDriver().is_ok());
}

}  // namespace
}  // namespace usb_fastboot_function
