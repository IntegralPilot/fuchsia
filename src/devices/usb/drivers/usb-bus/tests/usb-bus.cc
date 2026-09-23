// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-bus/usb-bus.h"

#include <fidl/fuchsia.hardware.usb.hci/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/usb/drivers/usb-bus/tests/common.h"

namespace usb_bus {

class BusTest : public zxtest::Test {
 public:
  BusTest() : hci_(nullptr) {}

  void SetUp() override {
    auto runtime_inst = fdf_testing::DriverRuntime::GetInstance();
    dispatcher_ = std::make_unique<fdf::UnownedSynchronizedDispatcher>(
        runtime_inst->StartBackgroundDispatcher());
    hci_dispatcher_ = std::make_unique<fdf::UnownedSynchronizedDispatcher>(
        runtime_inst->StartBackgroundDispatcher());
    hci_ = std::make_unique<FakeHci>((*hci_dispatcher_)->async_dispatcher());

    parent_->AddProtocol(ZX_PROTOCOL_USB_HCI, hci_->proto()->ops, hci_->proto()->ctx);

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    zx_status_t out_status = ZX_OK;
    auto result_out = fdf::RunOnDispatcherSync(
        (*hci_dispatcher_)->async_dispatcher(),
        [this, server_end = std::move(endpoints.server), &out_status]() mutable {
          outgoing_.emplace((*hci_dispatcher_)->async_dispatcher());
          zx::result<> result = outgoing_->AddService<fuchsia_hardware_usb_hci::UsbHciService>(
              fuchsia_hardware_usb_hci::UsbHciService::InstanceHandler({
                  .device =
                      [this](fidl::ServerEnd<fuchsia_hardware_usb_hci::UsbHci> server_end) {
                        fidl::BindServer((*hci_dispatcher_)->async_dispatcher(),
                                         std::move(server_end), hci_.get());
                      },
              }));
          if (result.is_error()) {
            out_status = result.status_value();
            return;
          }
          result = outgoing_->Serve(std::move(server_end));
          if (result.is_error()) {
            out_status = result.status_value();
            return;
          }
        });
    ASSERT_OK(result_out);
    ASSERT_OK(out_status);
    parent_->AddFidlService(fuchsia_hardware_usb_hci::UsbHciService::Name,
                            std::move(endpoints.client));

    auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [this]() {
      zx_status_t status = UsbBus::Create(nullptr, parent_.get());
      if (status != ZX_OK) {
        return;
      }
      auto* bus_dev = parent_->GetLatestChild();
      bus_ = bus_dev->GetDeviceContext<UsbBus>();
    });
    ASSERT_OK(result);
  }

  void TearDown() override {
    [[maybe_unused]] auto result =
        fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [this]() {
          auto* bus_dev = parent_->GetLatestChild();
          if (bus_dev) {
            device_async_remove(bus_dev);
          }
          mock_ddk::ReleaseFlaggedDevices(parent_.get());
        });
    ASSERT_OK(result);

    [[maybe_unused]] auto result2 =
        fdf::RunOnDispatcherSync((*hci_dispatcher_)->async_dispatcher(), [this]() {
          hci_.reset();
          outgoing_.reset();
        });
    ASSERT_OK(result2);
  }

 protected:
  std::shared_ptr<MockDevice> parent_{MockDevice::FakeRootParent()};
  fdf_testing::DriverRuntime* runtime() { return fdf_testing::DriverRuntime::GetInstance(); }
  std::unique_ptr<fdf::UnownedSynchronizedDispatcher> dispatcher_;
  std::unique_ptr<fdf::UnownedSynchronizedDispatcher> hci_dispatcher_;
  std::unique_ptr<FakeHci> hci_;
  std::optional<component::OutgoingDirectory> outgoing_;
  UsbBus* bus_;

  void AddDeviceFidl(uint32_t device_id, uint32_t hub_id,
                     fuchsia_hardware_usb_descriptor::UsbSpeed speed) {
    libsync::Completion completion;
    auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
      hci_->hci_interface_client()
          ->AddDevice(device_id, hub_id, speed)
          .ThenExactlyOnce(
              [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::AddDevice>&
                      response) {
                EXPECT_TRUE(response.ok());
                EXPECT_TRUE(response->is_ok());
                completion.Signal();
              });
    });
    ASSERT_OK(result);
    completion.Wait();
  }
};

TEST_F(BusTest, FidlAddDevice) {
  libsync::Completion completion;
  auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    hci_->hci_interface_client()
        ->AddDevice(2, 0, fuchsia_hardware_usb_descriptor::UsbSpeed::kFull)
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::AddDevice>&
                    response) {
              EXPECT_TRUE(response.ok());
              EXPECT_TRUE(response->is_ok());
              completion.Signal();
            });
  });
  ASSERT_OK(result);
  completion.Wait();

  auto* bus_dev = parent_->GetLatestChild();
  auto* usb_dev = bus_dev->GetLatestChild();
  ASSERT_NOT_NULL(usb_dev);
}

TEST_F(BusTest, FidlRemoveDeviceNotFound) {
  libsync::Completion completion;
  auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    // ID 100 out of range and should return an error.
    hci_->hci_interface_client()->RemoveDevice(100).ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::RemoveDevice>&
                response) {
          EXPECT_TRUE(response.ok());
          EXPECT_EQ(response->error_value(), ZX_ERR_INVALID_ARGS);
          completion.Signal();
        });
  });
  ASSERT_OK(result);
  completion.Wait();
}

TEST_F(BusTest, FidlRemoveDeviceNotPresent) {
  libsync::Completion completion;
  auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    // ID 5 is in range but no device has been added there.
    hci_->hci_interface_client()->RemoveDevice(5).ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::RemoveDevice>&
                response) {
          EXPECT_TRUE(response.ok());
          EXPECT_EQ(response->error_value(), ZX_ERR_BAD_STATE);
          completion.Signal();
        });
  });
  ASSERT_OK(result);
  completion.Wait();
}

TEST_F(BusTest, FidlRemoveDeviceAlreadyInProgress) {
  AddDeviceFidl(6, 0, fuchsia_hardware_usb_descriptor::UsbSpeed::kFull);

  libsync::Completion completion1;
  libsync::Completion completion2;

  auto result1 = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    hci_->hci_interface_client()->RemoveDevice(6).ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::RemoveDevice>&
                response) {
          EXPECT_TRUE(response.ok());
          EXPECT_TRUE(response->is_ok());
          completion1.Signal();
        });
  });
  ASSERT_OK(result1);

  auto result2 = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    // This second call should fail immediately with ZX_ERR_BAD_STATE
    hci_->hci_interface_client()->RemoveDevice(6).ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::RemoveDevice>&
                response) {
          EXPECT_TRUE(response.ok());
          EXPECT_EQ(response->error_value(), ZX_ERR_BAD_STATE);
          completion2.Signal();
        });
  });
  ASSERT_OK(result2);

  // completion2 should signal quickly
  completion2.Wait();

  // Trigger completion of removal to finish completion1
  auto* bus_dev = parent_->GetLatestChild();
  auto* usb_dev = bus_dev->GetLatestChild();
  ASSERT_NOT_NULL(usb_dev);

  runtime()->RunUntilIdle();
  auto result_rel = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(),
                                             [&] { mock_ddk::ReleaseFlaggedDevices(bus_dev); });
  ASSERT_OK(result_rel);

  completion1.Wait();
  ASSERT_EQ(bus_dev->child_count(), 0);
}

TEST_F(BusTest, FidlResetPort) {
  AddDeviceFidl(3, 0, fuchsia_hardware_usb_descriptor::UsbSpeed::kFull);

  libsync::Completion completion;
  auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    hci_->hci_interface_client()
        ->ResetPort(3, 1, true)
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::ResetPort>&
                    response) {
              EXPECT_TRUE(response.ok());
              // It should return ZX_ERR_BAD_STATE because it's not a hub.
              EXPECT_EQ(response->error_value(), ZX_ERR_BAD_STATE);
              completion.Signal();
            });
  });
  ASSERT_OK(result);
  completion.Wait();
}

TEST_F(BusTest, FidlReinitializeDevice) {
  AddDeviceFidl(4, 0, fuchsia_hardware_usb_descriptor::UsbSpeed::kFull);

  auto* bus_dev = parent_->GetLatestChild();
  auto* usb_dev = bus_dev->GetLatestChild();
  ASSERT_NOT_NULL(usb_dev);

  libsync::Completion completion;
  auto result2 = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    hci_->hci_interface_client()->ReinitializeDevice(4).ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_usb_hci::UsbHciInterface::ReinitializeDevice>&
                response) {
          EXPECT_TRUE(response.ok());
          EXPECT_TRUE(response->is_ok());
          completion.Signal();
        });
  });
  ASSERT_OK(result2);

  runtime()->RunUntilIdle();

  // Wait for the device to be flagged for removal by ReinitializeDevice processing.
  ASSERT_OK(usb_dev->WaitUntilAsyncRemoveCalled());

  auto result4 = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(),
                                          [&] { mock_ddk::ReleaseFlaggedDevices(bus_dev); });
  ASSERT_OK(result4);

  runtime()->RunUntilIdle();
  completion.Wait();

  // After re-initialization, exactly one new device should have been added.
  ASSERT_EQ(bus_dev->child_count(), 1);
  auto* usb_dev_new = bus_dev->GetLatestChild();
  ASSERT_NOT_NULL(usb_dev_new);
  ASSERT_NE(usb_dev, usb_dev_new);
}

TEST_F(BusTest, BanjoRemoveDevice) {
  // 1. Add a device via Banjo
  auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [this]() {
    [[maybe_unused]] auto status = hci_->bus_intf().AddDevice(1, 0, USB_SPEED_FULL);
  });
  ASSERT_OK(result);

  // Find the added bus device in mock-ddk
  auto* bus_dev = parent_->GetLatestChild();
  ASSERT_EQ(std::string(bus_dev->name()), "usb-bus");

  // Find the added usb device (it's a child of usb-bus)
  auto* usb_dev = bus_dev->GetLatestChild();
  ASSERT_NOT_NULL(usb_dev);

  // 2. Call RemoveDevice via Banjo.
  // This notifies UsbBus which in turn calls device_async_remove on the usb-device.
  auto result2 = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
    [[maybe_unused]] auto status = hci_->bus_intf().RemoveDevice(1);
    ASSERT_OK(status);
  });
  ASSERT_OK(result2);

  runtime()->RunUntilIdle();
  auto result4 = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(),
                                          [&] { mock_ddk::ReleaseFlaggedDevices(bus_dev); });
  ASSERT_OK(result4);

  runtime()->RunUntilIdle();
  ASSERT_EQ(bus_dev->child_count(), 0);
}

}  // namespace usb_bus
