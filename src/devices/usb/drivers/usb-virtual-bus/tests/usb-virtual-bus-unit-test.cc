// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.usb.dci/cpp/test_base.h>
#include <fidl/fuchsia.hardware.usb.hci/cpp/test_base.h>
#include <fuchsia/hardware/usb/bus/cpp/banjo.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/sync/completion.h>

#include <gtest/gtest.h>

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-bus.h"

namespace usb_virtual_bus {
namespace {

namespace fdci = fuchsia_hardware_usb_dci;
namespace fhci = fuchsia_hardware_usb_hci;
namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace frequest = fuchsia_hardware_usb_request;

const size_t kMaxPacketSize = 16;

class FakeUsbBus : public fidl::testing::TestBase<fhci::UsbHciInterface> {
 public:
  void Bind(fidl::ServerEnd<fhci::UsbHciInterface> server_end) {
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server_end),
                         this, fidl::kIgnoreBindingClosure);
  }

  void set_remove_device_status(zx_status_t status) { remove_device_status_ = status; }
  void set_add_device_status(zx_status_t status) { add_device_status_ = status; }

  void set_stall_add_device(bool stall) { stall_add_device_ = stall; }
  void signal_add_device() {
    if (saved_add_device_completer_) {
      saved_add_device_completer_->Reply(zx::make_result(add_device_status_));
      saved_add_device_completer_.reset();
    }
  }

  void set_stall_remove_device(bool stall) { stall_remove_device_ = stall; }
  void signal_remove_device() {
    if (saved_remove_device_completer_) {
      saved_remove_device_completer_->Reply(zx::make_result(remove_device_status_));
      saved_remove_device_completer_.reset();
    }
  }

  void AddDevice(AddDeviceRequest& request, AddDeviceCompleter::Sync& completer) override {
    if (stall_add_device_) {
      saved_add_device_completer_ = completer.ToAsync();
      return;
    }
    completer.Reply(zx::make_result(add_device_status_));
  }
  void RemoveDevice(RemoveDeviceRequest& request, RemoveDeviceCompleter::Sync& completer) override {
    if (stall_remove_device_) {
      saved_remove_device_completer_ = completer.ToAsync();
      return;
    }
    completer.Reply(zx::make_result(remove_device_status_));
  }
  void ResetPort(ResetPortRequest& request, ResetPortCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  void ReinitializeDevice(ReinitializeDeviceRequest& request,
                          ReinitializeDeviceCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  bool has_saved_add_device_completer() const { return saved_add_device_completer_.has_value(); }
  bool has_saved_remove_device_completer() const {
    return saved_remove_device_completer_.has_value();
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {}

 private:
  fidl::ServerBindingGroup<fhci::UsbHciInterface> bindings_;
  zx_status_t remove_device_status_ = ZX_OK;
  zx_status_t add_device_status_ = ZX_OK;
  bool stall_add_device_ = false;
  std::optional<AddDeviceCompleter::Async> saved_add_device_completer_;
  bool stall_remove_device_ = false;
  std::optional<RemoveDeviceCompleter::Async> saved_remove_device_completer_;
};

class EndpointHandler : public fidl::SyncEventHandler<fendpoint::Endpoint> {
 public:
  ~EndpointHandler() { EXPECT_TRUE(expected_on_completion_.empty()); }

  using ExpectedOnCompletionFnType =
      std::function<void(fidl::Event<fendpoint::Endpoint::OnCompletion>& event)>;

  void OnCompletion(fidl::Event<fendpoint::Endpoint::OnCompletion>& event) override {
    ASSERT_FALSE(expected_on_completion_.empty());
    expected_on_completion_.front()(event);
    expected_on_completion_.pop();
  }

  void ExpectOnCompletion(ExpectedOnCompletionFnType fn) {
    expected_on_completion_.emplace(std::move(fn));
  }

  // Helper functions for common expected OnCompletions
  static ExpectedOnCompletionFnType ExpectOnCompletionDirect(size_t data_size, bool validate_data) {
    return [data_size, validate_data](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
      ASSERT_EQ(event.completion().size(), 1UL);
      ASSERT_EQ(event.completion()[0].status(), ZX_OK);
      ASSERT_EQ(event.completion()[0].transfer_size(), data_size);
      if (validate_data) {
        const auto& data = *event.completion()[0].request()->data();
        ASSERT_EQ(data.size(), 1UL);
        ASSERT_EQ(data[0].offset(), 0UL);
        ASSERT_EQ(data[0].buffer()->Which(), frequest::Buffer::Tag::kData);
        for (size_t i = 0; i < data_size; i++) {
          ASSERT_EQ(data[0].buffer()->data().value()[i], static_cast<uint8_t>(i));
        }
      }
    };
  }

  static ExpectedOnCompletionFnType ExpectOnCompletionVmo(size_t data_size,
                                                          uint8_t* data_ptr = nullptr) {
    return [data_size, data_ptr](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
      ASSERT_EQ(event.completion().size(), 1UL);
      ASSERT_EQ(event.completion()[0].status(), ZX_OK);
      ASSERT_EQ(event.completion()[0].transfer_size(), data_size);
      if (data_ptr) {
        const auto& data = *event.completion()[0].request()->data();
        ASSERT_EQ(data.size(), 1UL);
        ASSERT_EQ(data[0].offset(), 0UL);
        ASSERT_EQ(data[0].buffer()->Which(), frequest::Buffer::Tag::kVmoId);
        ASSERT_EQ(data[0].buffer()->vmo_id().value(), 1UL);
        for (size_t i = 0; i < data_size; i++) {
          ASSERT_EQ(data_ptr[i], static_cast<uint8_t>(i));
        }
      }
    };
  }

 private:
  std::queue<ExpectedOnCompletionFnType> expected_on_completion_;
};

class FakeDci : public fidl::testing::TestBase<fdci::UsbDciInterface> {
 public:
  ~FakeDci() { EXPECT_EQ(expected_control_, 0UL); }

  // FakeEndpoint& endpoint() { return endpoint_; }
  // void WaitForUnbind() { sync_completion_wait(&unbind_sync_, ZX_TIME_INFINITE); }
  fidl::ClientEnd<fdci::UsbDciInterface> Connect() {
    auto [client_end, server_end] = fidl::Endpoints<fdci::UsbDciInterface>::Create();
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server_end),
                         this, fidl::kIgnoreBindingClosure);
    return std::move(client_end);
  }

  void ExpectControl() { expected_control_++; }
  libsync::Completion& wait_for_control() { return wait_for_control_; }

  void set_set_connected_status(zx_status_t status) { set_connected_status_ = status; }

  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override {
    if (!expected_control_) {
      // Used for Disconnect and unbind tests. Store the completer.
      store_completer_.emplace(completer.ToAsync());
      wait_for_control_.Signal();
      return;
    }
    expected_control_--;

    std::vector<uint8_t> control_data(request.setup().w_length());
    for (size_t i = 0; i < request.setup().w_length(); i++) {
      control_data[i] = static_cast<uint8_t>(i);
    }
    completer.Reply(zx::ok(std::move(control_data)));
  }
  void SetConnected(SetConnectedRequest& request, SetConnectedCompleter::Sync& completer) override {
    store_completer_.reset();
    completer.Reply(zx::make_result(set_connected_status_));
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fdci::UsbDciInterface> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    ASSERT_FALSE(true);
  }
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}

 private:
  fidl::ServerBindingGroup<fdci::UsbDciInterface> bindings_;
  std::atomic_uint32_t expected_control_ = 0;
  std::optional<ControlCompleter::Async> store_completer_;
  libsync::Completion wait_for_control_;
  zx_status_t set_connected_status_ = ZX_OK;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override { return zx::ok(); }

  FakeDci dci_;
  FakeUsbBus usb_bus_;
};

class UsbVirtualBusTestConfig final {
 public:
  using DriverType = UsbVirtualBus;
  using EnvironmentType = TestEnvironment;
};

class UsbVirtualBusTest : public testing::Test {
 protected:
  void SetUp() override {
    ASSERT_TRUE(driver_test_.StartDriver().is_ok());

    zx::result connect_result =
        driver_test().ConnectThroughDevfs<fuchsia_hardware_usb_virtual_bus::Bus>("usb-virtual-bus");
    EXPECT_TRUE(connect_result.is_ok());
    ASSERT_TRUE(connect_result->is_valid());
    virtual_bus_async_.Bind(std::move(*connect_result),
                            fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  void TearDown() override { ASSERT_TRUE(driver_test_.StopDriver().is_ok()); }

  zx_status_t ManualOnStartDci() {
    libsync::Completion done;
    zx_status_t out_status;
    driver_test().RunInDriverContext([&](UsbVirtualBus& driver) {
      driver.OnStartDci([&](zx_status_t status) {
        out_status = status;
        done.Signal();
      });
    });
    done.Wait();
    return out_status;
  }

  zx_status_t ManualOnStopDci() {
    libsync::Completion done;
    zx_status_t out_status;
    driver_test().RunInDriverContext([&](UsbVirtualBus& driver) {
      driver.OnStopDci([&](zx_status_t status) {
        out_status = status;
        done.Signal();
      });
    });
    done.Wait();
    return out_status;
  }

  UsbVirtualBus::ConnectedState GetState() {
    UsbVirtualBus::ConnectedState state;
    driver_test().RunInDriverContext(
        [&](UsbVirtualBus& driver) { state = driver.GetConnectedState(); });
    return state;
  }

  void EnableAndConnect() {
    Enable();
    Connect();
  }

  void Enable() {
    auto enable_result = virtual_bus().sync()->Enable();
    ASSERT_TRUE(enable_result.ok());
    ASSERT_EQ(enable_result->status, ZX_OK);

    auto [client_end, server_end] = fidl::Endpoints<fhci::UsbHciInterface>::Create();
    driver_test().RunInEnvironmentTypeContext(
        [&](TestEnvironment& env) { env.usb_bus_.Bind(std::move(server_end)); });
    driver_test().RunInDriverContext([&](UsbVirtualBus& driver) {
      ASSERT_TRUE(driver.SetBusInterface(std::move(client_end)).is_ok());
    });
    fidl::ClientEnd<fdci::UsbDciInterface> dci_client =
        driver_test().RunInEnvironmentTypeContext<fidl::ClientEnd<fdci::UsbDciInterface>>(
            [](TestEnvironment& env) { return env.dci_.Connect(); });
    driver_test().RunInDriverContext([&](UsbVirtualBus& driver) {
      zx::result result = driver.SetDciInterface(std::move(dci_client));
      ASSERT_TRUE(result.is_ok());
    });
  }

  // Helper to simulate a full connection sequence.
  void Connect() {
    // Manually trigger OnStartDci in the driver context. This simulates
    // the completion of the device-side (DCI) initialization which normally
    // happens when the peripheral driver calls StartController.
    EXPECT_EQ(ManualOnStartDci(), ZX_OK);

    auto connect_result_fidl = virtual_bus().sync()->Connect();
    ASSERT_TRUE(connect_result_fidl.ok());
    ASSERT_EQ(connect_result_fidl->status, ZX_OK);
  }

  template <typename Service>
  fidl::ClientEnd<fendpoint::Endpoint> ConnectToEndpoint(uint8_t ep_addr) {
    auto controller = driver_test().Connect<typename Service::Device>();
    EXPECT_TRUE(controller.is_ok());

    if constexpr (std::is_same<Service, fdci::UsbDciService>::value) {
      // Device needs to configure endpoint and set max_packet_size_
      auto result =
          fidl::WireCall(*controller)
              ->ConfigureEndpoint(
                  {.b_endpoint_address = ep_addr, .w_max_packet_size = kMaxPacketSize}, {});
      EXPECT_TRUE(result.ok());
      EXPECT_TRUE(result->is_ok());
    }

    auto [client_end, server_end] = fidl::Endpoints<fendpoint::Endpoint>::Create();
    if constexpr (std::is_same<Service, fdci::UsbDciService>::value) {
      auto result = fidl::WireCall(*controller)->ConnectToEndpoint(ep_addr, std::move(server_end));
      EXPECT_TRUE(result.ok());
      EXPECT_TRUE(result->is_ok());
    } else {
      auto result =
          fidl::WireCall(*controller)->ConnectToEndpoint(0, ep_addr, std::move(server_end));
      EXPECT_TRUE(result.ok());
      EXPECT_TRUE(result->is_ok());
    };
    return std::move(client_end);
  }

  // Registers one VMO
  std::pair<uint8_t*, zx::vmo> RegisterVmo(fidl::SyncClient<fendpoint::Endpoint>& client,
                                           size_t data_size, size_t id = 1) {
    auto result = client->RegisterVmos(
        std::vector<fendpoint::VmoInfo>{{fendpoint::VmoInfo().id(id).size(data_size)}});
    EXPECT_TRUE(result.is_ok());
    EXPECT_EQ(result->vmos().size(), 1UL);
    EXPECT_EQ(result->vmos()[0].id(), id);
    zx::vmo vmo = std::move(*result->vmos()[0].vmo());
    zx_vaddr_t mapped_addr;
    EXPECT_EQ(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, data_size,
                                         &mapped_addr),
              ZX_OK);
    return {reinterpret_cast<uint8_t*>(mapped_addr), std::move(vmo)};
  }

  fdf_testing::BackgroundDriverTest<UsbVirtualBusTestConfig>& driver_test() { return driver_test_; }
  fidl::WireSharedClient<fuchsia_hardware_usb_virtual_bus::Bus>& virtual_bus() {
    return virtual_bus_async_;
  }

 private:
  fdf_testing::BackgroundDriverTest<UsbVirtualBusTestConfig> driver_test_;
  fidl::WireSharedClient<fuchsia_hardware_usb_virtual_bus::Bus> virtual_bus_async_;
};

TEST_F(UsbVirtualBusTest, LifecycleTest) {
  // The driver should create a child node for the bus itself.
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(1UL, node.children().size()); });
}

TEST_F(UsbVirtualBusTest, EnableDisableTest) {
  auto enable_result = virtual_bus().sync()->Enable();
  ASSERT_TRUE(enable_result.ok());
  ASSERT_EQ(enable_result->status, ZX_OK);

  // After enabling, there should be two additional children: host and device.
  // The total children will be 3 (bus, host, device).
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(3UL, node.children().size()); });

  auto disable_result = virtual_bus().sync()->Disable();
  ASSERT_TRUE(disable_result.ok());
  ASSERT_EQ(disable_result->status, ZX_OK);

  // After disabling, the host and device children should eventually go away.
  while (driver_test().RunInNodeContext<size_t>(
             [](auto& node) { return node.children().size(); }) != 1) {
    zx::nanosleep(zx::deadline_after(zx::usec(30)));
  }
}

TEST_F(UsbVirtualBusTest, ReconnectTest) {
  EnableAndConnect();
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kConnected);

  auto disconnect_result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(disconnect_result.ok());
  ASSERT_EQ(disconnect_result->status, ZX_OK);
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);

  // ManualOnStartDci is a DCI-side event (peripheral driver starting).
  // In the current model, this triggers the initial connection sequence (ConnectInternal).
  EXPECT_EQ(ManualOnStartDci(), ZX_OK);

  auto connect_result_fidl = virtual_bus().sync()->Connect();
  ASSERT_TRUE(connect_result_fidl.ok());
  ASSERT_EQ(connect_result_fidl->status, ZX_OK);
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kConnected);
}

TEST_F(UsbVirtualBusTest, ConcurrentConnectOverlapTest) {
  Enable();

  // 1. Stall AddDevice in the fake bus.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.set_stall_add_device(true); });

  // 2. Start connection asynchronously. It will stall in AddDevice.
  bool connect_done = false;
  virtual_bus()->Connect().Then(
      [&](fidl::WireUnownedResult<fuchsia_hardware_usb_virtual_bus::Bus::Connect>& result) {
        ASSERT_TRUE(result.ok());
        connect_done = true;
      });

  // Busy wait until the driver context has processed the ConnectInternal call.
  // We use RunUntil with a simple check to advance the dispatcher.
  driver_test().runtime().RunUntil(
      [&]() { return GetState() == UsbVirtualBus::ConnectedState::kConnecting; });
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kConnecting);

  // 3. Trigger disconnect while connecting - should still return BAD_STATE
  // for now as cross-operation overlap (Connect vs Disconnect) is rejected.
  {
    ASSERT_FALSE(connect_done);
    auto result = virtual_bus().sync()->Disconnect();
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result->status, ZX_ERR_BAD_STATE);
  }

  // 4. Trigger another connect while connecting - should now be queued and return ZX_OK.
  bool connect_2_done = false;
  virtual_bus()->Connect().Then(
      [&](fidl::WireUnownedResult<fuchsia_hardware_usb_virtual_bus::Bus::Connect>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_EQ(result->status, ZX_OK);
        connect_2_done = true;
      });

  // Wait for the staged ConnectInternal to reach the fake bus.
  driver_test().runtime().RunUntil([&]() {
    return driver_test().RunInEnvironmentTypeContext<bool>(
        [](TestEnvironment& env) { return env.usb_bus_.has_saved_add_device_completer(); });
  });

  // 5. Unstall AddDevice and wait for both connections to complete.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.signal_add_device(); });

  driver_test().runtime().RunUntil([&]() { return connect_done && connect_2_done; });
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kConnected);
}

TEST_F(UsbVirtualBusTest, ConcurrentDisconnectOverlapTest) {
  EnableAndConnect();

  // 1. Stall RemoveDevice in the fake bus.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.set_stall_remove_device(true); });

  // 2. Start disconnect asynchronously. It will stall in RemoveDevice.
  bool disconnect_done = false;
  virtual_bus()->Disconnect().Then(
      [&](fidl::WireUnownedResult<fuchsia_hardware_usb_virtual_bus::Bus::Disconnect>& result) {
        ASSERT_TRUE(result.ok());
        disconnect_done = true;
      });

  driver_test().runtime().RunUntil(
      [&]() { return GetState() == UsbVirtualBus::ConnectedState::kDisconnecting; });
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnecting);

  // 3. Trigger connect while disconnecting - should return BAD_STATE.
  {
    ASSERT_FALSE(disconnect_done);
    auto result = virtual_bus().sync()->Connect();
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result->status, ZX_ERR_BAD_STATE);
  }

  // 4. Trigger another disconnect while disconnecting - should now be queued and return ZX_OK.
  bool disconnect_2_done = false;
  virtual_bus()->Disconnect().Then(
      [&](fidl::WireUnownedResult<fuchsia_hardware_usb_virtual_bus::Bus::Disconnect>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_EQ(result->status, ZX_OK);
        disconnect_2_done = true;
      });

  // Wait for the staged DisconnectInternal to reach the fake bus.
  driver_test().runtime().RunUntil([&]() {
    return driver_test().RunInEnvironmentTypeContext<bool>(
        [](TestEnvironment& env) { return env.usb_bus_.has_saved_remove_device_completer(); });
  });

  // 5. Unstall RemoveDevice and wait for both to complete.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.signal_remove_device(); });

  driver_test().runtime().RunUntil([&]() { return disconnect_done && disconnect_2_done; });
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);
}

TEST_F(UsbVirtualBusTest, DciInitiatedConnectOverlapTest) {
  Enable();

  // 1. Stall AddDevice in the fake bus.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.set_stall_add_device(true); });

  // 2. Peripheral driver starts dci.
  ASSERT_EQ(ManualOnStartDci(), ZX_OK);
  driver_test().runtime().RunUntil(
      [&]() { return GetState() == UsbVirtualBus::ConnectedState::kConnecting; });

  // 3. User also calls Connect via FIDL.
  bool fidl_connect_done = false;
  virtual_bus()->Connect().Then(
      [&](fidl::WireUnownedResult<fuchsia_hardware_usb_virtual_bus::Bus::Connect>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_EQ(result->status, ZX_OK);
        fidl_connect_done = true;
      });

  // 4. Verify both are pending.
  driver_test().runtime().RunUntil(
      [&]() { return GetState() == UsbVirtualBus::ConnectedState::kConnecting; });
  ASSERT_FALSE(fidl_connect_done);

  // 5. Unstall and verify both finish successfully.
  // Wait for the staged ConnectInternal to reach the fake bus.
  driver_test().runtime().RunUntil([&]() {
    return driver_test().RunInEnvironmentTypeContext<bool>(
        [](TestEnvironment& env) { return env.usb_bus_.has_saved_add_device_completer(); });
  });

  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.signal_add_device(); });

  driver_test().runtime().RunUntil([&]() { return fidl_connect_done; });
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kConnected);
}

TEST_F(UsbVirtualBusTest, ConnectHciErrorTeardownTest) {
  Enable();
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);

  // 1. Simulate an error in HCI AddDevice.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.set_add_device_status(ZX_ERR_INTERNAL); });

  // 2. Trigger connect.
  auto connect_result_fidl = virtual_bus().sync()->Connect();
  // Connect should fail with the error status.
  ASSERT_TRUE(connect_result_fidl.ok());
  ASSERT_EQ(connect_result_fidl->status, ZX_ERR_INTERNAL);

  // Verify rollback.
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);

  // 3. Mark DCI as ready.
  // ManualOnStartDci (OnStartDci) returns OK immediately because it starts the sequence in
  // background.
  EXPECT_EQ(ManualOnStartDci(), ZX_OK);

  // We wait for the background connection to fail by calling Connect().
  auto third_connect = virtual_bus().sync()->Connect();
  ASSERT_TRUE(third_connect.ok());
  ASSERT_EQ(third_connect->status, ZX_ERR_INTERNAL);
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);

  // Subsequent connect should work if we fix the environment status.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.set_add_device_status(ZX_OK); });

  auto second_connect = virtual_bus().sync()->Connect();
  ASSERT_TRUE(second_connect.ok());
  ASSERT_EQ(second_connect->status, ZX_OK);
}

TEST_F(UsbVirtualBusTest, UninitializedErrorTest) {
  // Bus is NOT enabled, so host_ and device_ are null.
  auto connect_result = virtual_bus().sync()->Connect();
  ASSERT_TRUE(connect_result.ok());
  ASSERT_EQ(connect_result->status, ZX_ERR_BAD_STATE);

  auto disconnect_result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(disconnect_result.ok());
  ASSERT_EQ(disconnect_result->status, ZX_ERR_BAD_STATE);
}

TEST_F(UsbVirtualBusTest, IdempotentSimulationTest) {
  EnableAndConnect();

  // Connect while already connected should return OK.
  auto connect_result = virtual_bus().sync()->Connect();
  ASSERT_TRUE(connect_result.ok());
  ASSERT_EQ(connect_result->status, ZX_OK);

  auto disconnect_result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(disconnect_result.ok());
  ASSERT_EQ(disconnect_result->status, ZX_OK);

  // Disconnect while already disconnected should return OK.
  auto second_disconnect = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(second_disconnect.ok());
  ASSERT_EQ(second_disconnect->status, ZX_OK);
}

TEST_F(UsbVirtualBusTest, DciSetConnectedErrorTest) {
  Enable();
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);

  // 1. Simulate an error in DCI SetConnected.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.dci_.set_set_connected_status(ZX_ERR_NOT_SUPPORTED); });

  // 2. Trigger OnStartDci. It returns OK immediately.
  EXPECT_EQ(ManualOnStartDci(), ZX_OK);

  // Use Connect() to wait for the background sequence to fail.
  auto connect_result = virtual_bus().sync()->Connect();
  ASSERT_TRUE(connect_result.ok());
  ASSERT_EQ(connect_result->status, ZX_ERR_INTERNAL);
  ASSERT_EQ(GetState(), UsbVirtualBus::ConnectedState::kDisconnected);

  // 3. Subsequent OnStartDci should work if we fix the status.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.dci_.set_set_connected_status(ZX_OK); });

  EXPECT_EQ(ManualOnStartDci(), ZX_OK);
}

TEST_F(UsbVirtualBusTest, DisconnectErrorPropagationTest) {
  EnableAndConnect();

  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.usb_bus_.set_remove_device_status(ZX_ERR_INTERNAL); });

  auto disconnect_result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(disconnect_result.ok());
  // The error should be propagated from RemoveDevice.
  ASSERT_EQ(disconnect_result->status, ZX_ERR_INTERNAL);
}

TEST_F(UsbVirtualBusTest, FidlControlRequestTest) {
  EnableAndConnect();

  fidl::SyncClient<fendpoint::Endpoint> ep_client(
      ConnectToEndpoint<fhci::UsbHciService>(static_cast<uint8_t>(0)));
  EndpointHandler event_handler;

  // Direct data transfer
  {
    std::vector<frequest::Request> requests;
    requests.emplace_back(std::move(frequest::Request().information(
        frequest::RequestInfo::WithControl(frequest::ControlRequestInfo().setup(
            fuchsia_hardware_usb_descriptor::UsbSetup()
                .bm_request_type(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)
                .b_request(USB_REQ_GET_DESCRIPTOR)
                .w_value(static_cast<uint16_t>(USB_DT_DEVICE << 8))
                .w_index(0)
                .w_length(sizeof(usb_device_descriptor_t)))))));

    driver_test().RunInEnvironmentTypeContext(
        [](TestEnvironment& env) { return env.dci_.ExpectControl(); });
    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());

    event_handler.ExpectOnCompletion(
        EndpointHandler::ExpectOnCompletionDirect(sizeof(usb_device_descriptor_t), true));
    ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
  }

  // VMO data transfer
  {
    auto [data_ptr, vmo] = RegisterVmo(ep_client, sizeof(usb_device_descriptor_t));

    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithVmoId(1))
        .offset(0)
        .size(sizeof(usb_device_descriptor_t));
    requests.emplace_back(std::move(
        frequest::Request()
            .information(frequest::RequestInfo::WithControl(frequest::ControlRequestInfo().setup(
                fuchsia_hardware_usb_descriptor::UsbSetup()
                    .bm_request_type(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)
                    .b_request(USB_REQ_GET_DESCRIPTOR)
                    .w_value(static_cast<uint16_t>(USB_DT_DEVICE << 8))
                    .w_index(0)
                    .w_length(sizeof(usb_device_descriptor_t)))))
            .data(std::move(buffer))));

    driver_test().RunInEnvironmentTypeContext(
        [](TestEnvironment& env) { return env.dci_.ExpectControl(); });
    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());

    event_handler.ExpectOnCompletion(
        EndpointHandler::ExpectOnCompletionVmo(sizeof(usb_device_descriptor_t), data_ptr));
    ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
  }
}

TEST_F(UsbVirtualBusTest, FidlOutRequestTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 1 | USB_DIR_OUT;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kDataSize = 14;
  auto [device_data, device_vmo] = RegisterVmo(device_ep, kDataSize);

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back().buffer(frequest::Buffer::WithVmoId(1)).offset(0).size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());
  }

  auto [host_data, host_vmo] = RegisterVmo(host_ep, kDataSize);
  for (size_t i = 0; i < kDataSize; i++) {
    static_cast<uint8_t*>(host_data)[i] = static_cast<uint8_t>(i);
  }

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back().buffer(frequest::Buffer::WithVmoId(1)).offset(0).size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  host_event_handler.ExpectOnCompletion(EndpointHandler::ExpectOnCompletionVmo(kDataSize));
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());

  device_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionVmo(kDataSize, device_data));
  ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
}

TEST_F(UsbVirtualBusTest, FidlInRequestTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kDataSize = kMaxPacketSize;
  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kDataSize)))
        .offset(0)
        .size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    std::vector<uint8_t> data(kDataSize);
    for (size_t i = 0; i < kDataSize; i++) {
      data[i] = static_cast<uint8_t>(i);
    }
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::move(data)))
        .offset(0)
        .size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());
  }

  host_event_handler.ExpectOnCompletion(EndpointHandler::ExpectOnCompletionDirect(kDataSize, true));
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());

  device_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionDirect(kDataSize, false));
  ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
}

TEST_F(UsbVirtualBusTest, FidlOutRequestUnderflowTransferTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 1 | USB_DIR_OUT;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kDeviceDataSize = 14;
  static constexpr size_t kHostDataSize = 12;
  auto [device_data, device_vmo] = RegisterVmo(device_ep, kDeviceDataSize);

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back().buffer(frequest::Buffer::WithVmoId(1)).offset(0).size(kDeviceDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());
  }

  auto [host_data, host_vmo] = RegisterVmo(host_ep, kHostDataSize);
  for (size_t i = 0; i < kHostDataSize; i++) {
    static_cast<uint8_t*>(host_data)[i] = static_cast<uint8_t>(i);
  }

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back().buffer(frequest::Buffer::WithVmoId(1)).offset(0).size(kHostDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  host_event_handler.ExpectOnCompletion(EndpointHandler::ExpectOnCompletionVmo(kHostDataSize));
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());

  device_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionVmo(kHostDataSize, device_data));
  ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
}

TEST_F(UsbVirtualBusTest, FidlInRequestShortTransferTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kHostDataSize = 18;
  static constexpr size_t kDeviceDataSize = 14;

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kHostDataSize)))
        .offset(0)
        .size(kHostDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    std::vector<uint8_t> data(kDeviceDataSize);
    for (size_t i = 0; i < kDeviceDataSize; i++) {
      data[i] = static_cast<uint8_t>(i);
    }
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::move(data)))
        .offset(0)
        .size(kDeviceDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());
  }

  host_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionDirect(kDeviceDataSize, true));
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());

  device_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionDirect(kDeviceDataSize, false));
  ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
}

TEST_F(UsbVirtualBusTest, FidlInRequestOverrunTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kHostDataSize = 14;
  static constexpr size_t kDeviceDataSize = 15;

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kHostDataSize)))
        .offset(0)
        .size(kHostDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    std::vector<uint8_t> data(kDeviceDataSize);
    for (size_t i = 0; i < kDeviceDataSize; i++) {
      data[i] = static_cast<uint8_t>(i);
    }
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::move(data)))
        .offset(0)
        .size(kDeviceDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());
  }

  auto expect_overrun = [](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
    ASSERT_EQ(event.completion().size(), 1UL);
    ASSERT_EQ(event.completion()[0].status(), ZX_ERR_IO_OVERRUN);
    ASSERT_EQ(event.completion()[0].transfer_size(), 0);
  };

  host_event_handler.ExpectOnCompletion(expect_overrun);
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());

  device_event_handler.ExpectOnCompletion(expect_overrun);
  ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
}

TEST_F(UsbVirtualBusTest, FidlOutRequestMultipleDeviceRequestsTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 1 | USB_DIR_OUT;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kHostDataSize = kMaxPacketSize + 5;
  static constexpr size_t kDeviceDataSize = kMaxPacketSize;

  auto [host_data, host_vmo] = RegisterVmo(host_ep, kHostDataSize);
  for (size_t i = 0; i < kHostDataSize; i++) {
    static_cast<uint8_t*>(host_data)[i] = static_cast<uint8_t>(i);
  }

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back().buffer(frequest::Buffer::WithVmoId(1)).offset(0).size(kHostDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));
    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  auto [device_data1, device_vmo1] = RegisterVmo(device_ep, kDeviceDataSize, 1);
  auto [device_data2, device_vmo2] = RegisterVmo(device_ep, kDeviceDataSize, 2);

  device_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionVmo(kDeviceDataSize, nullptr));
  device_event_handler.ExpectOnCompletion(EndpointHandler::ExpectOnCompletionVmo(5, nullptr));
  for (size_t i = 0; i < 2; i++) {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithVmoId(i + 1))
        .offset(0)
        .size(kDeviceDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));
    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());

    ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
  }

  host_event_handler.ExpectOnCompletion(EndpointHandler::ExpectOnCompletionVmo(kHostDataSize));
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());

  // Verify data
  for (size_t i = 0; i < kDeviceDataSize; i++) {
    ASSERT_EQ(device_data1[i], static_cast<uint8_t>(i));
  }
  for (size_t i = 0; i < 5; i++) {
    ASSERT_EQ(device_data2[i], static_cast<uint8_t>(i + kDeviceDataSize));
  }
}

TEST_F(UsbVirtualBusTest, FidlInRequestMultipleDeviceRequestsTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> host_ep(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler host_event_handler;
  fidl::SyncClient<fendpoint::Endpoint> device_ep(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler device_event_handler;

  static constexpr size_t kHostDataSize = kMaxPacketSize + 5;
  static constexpr size_t kDeviceDataSize = kMaxPacketSize;

  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kHostDataSize)))
        .offset(0)
        .size(kHostDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));
    ASSERT_TRUE(host_ep->QueueRequests(std::move(requests)).is_ok());
  }

  size_t kDataTransferSize[] = {kDeviceDataSize, 5};
  for (size_t i = 0; i < 2; i++) {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    std::vector<uint8_t> data(kDataTransferSize[i]);
    for (size_t j = 0; j < kDataTransferSize[i]; j++) {
      data[j] = static_cast<uint8_t>(i * kDeviceDataSize + j);
    }
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::move(data)))
        .offset(0)
        .size(kDataTransferSize[i]);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));
    ASSERT_TRUE(device_ep->QueueRequests(std::move(requests)).is_ok());

    device_event_handler.ExpectOnCompletion(
        EndpointHandler::ExpectOnCompletionDirect(kDataTransferSize[i], false));
    ASSERT_TRUE(device_ep.HandleOneEvent(device_event_handler).ok());
  }

  host_event_handler.ExpectOnCompletion(
      EndpointHandler::ExpectOnCompletionDirect(kHostDataSize, true));
  ASSERT_TRUE(host_ep.HandleOneEvent(host_event_handler).ok());
}

TEST_F(UsbVirtualBusTest, QueueControlRequestBeforeConnectTest) {
  auto enable_result = virtual_bus().sync()->Enable();
  ASSERT_TRUE(enable_result.ok());
  ASSERT_EQ(enable_result->status, ZX_OK);

  fidl::SyncClient<fendpoint::Endpoint> ep_client(
      ConnectToEndpoint<fhci::UsbHciService>(static_cast<uint8_t>(0)));
  EndpointHandler event_handler;

  {
    std::vector<frequest::Request> requests;
    requests.emplace_back(std::move(frequest::Request().information(
        frequest::RequestInfo::WithControl(frequest::ControlRequestInfo().setup(
            fuchsia_hardware_usb_descriptor::UsbSetup()
                .bm_request_type(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)
                .b_request(USB_REQ_GET_DESCRIPTOR)
                .w_value(static_cast<uint16_t>(USB_DT_DEVICE << 8))
                .w_index(0)
                .w_length(sizeof(usb_device_descriptor_t)))))));

    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());
  }

  event_handler.ExpectOnCompletion([](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
    ASSERT_EQ(event.completion().size(), 1UL);
    ASSERT_EQ(event.completion()[0].status(), ZX_ERR_IO_NOT_PRESENT);
    ASSERT_EQ(event.completion()[0].transfer_size(), 0);
  });
  ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
}

TEST_F(UsbVirtualBusTest, QueueNormalRequestBeforeConnectedTest) {
  auto enable_result = virtual_bus().sync()->Enable();
  ASSERT_TRUE(enable_result.ok());
  ASSERT_EQ(enable_result->status, ZX_OK);

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> ep_client(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler event_handler;

  static constexpr size_t kDataSize = 14;
  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kDataSize)))
        .offset(0)
        .size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());
  }

  event_handler.ExpectOnCompletion([](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
    ASSERT_EQ(event.completion().size(), 1UL);
    ASSERT_EQ(event.completion()[0].status(), ZX_ERR_IO_NOT_PRESENT);
    ASSERT_EQ(event.completion()[0].transfer_size(), 0);
  });
  ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
}

TEST_F(UsbVirtualBusTest, UnexpectedDisconnectDuringControlTest) {
  EnableAndConnect();

  fidl::SyncClient<fendpoint::Endpoint> ep_client(
      ConnectToEndpoint<fhci::UsbHciService>(static_cast<uint8_t>(0)));
  EndpointHandler event_handler;

  {
    std::vector<frequest::Request> requests;
    requests.emplace_back(std::move(frequest::Request().information(
        frequest::RequestInfo::WithControl(frequest::ControlRequestInfo().setup(
            fuchsia_hardware_usb_descriptor::UsbSetup()
                .bm_request_type(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)
                .b_request(USB_REQ_GET_DESCRIPTOR)
                .w_value(static_cast<uint16_t>(USB_DT_DEVICE << 8))
                .w_index(0)
                .w_length(sizeof(usb_device_descriptor_t)))))));

    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());
  }

  libsync::Completion* wait;
  driver_test().RunInEnvironmentTypeContext(
      [&wait](TestEnvironment& env) { wait = &env.dci_.wait_for_control(); });
  wait->Wait();

  auto result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result->status, ZX_OK);

  event_handler.ExpectOnCompletion([](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
    ASSERT_EQ(event.completion().size(), 1UL);
    ASSERT_EQ(event.completion()[0].status(), ZX_ERR_IO_NOT_PRESENT);
    ASSERT_EQ(event.completion()[0].transfer_size(), 0);
  });
  ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
}

TEST_F(UsbVirtualBusTest, UnexpectedDisconnectDuringHostNormalTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> ep_client(ConnectToEndpoint<fhci::UsbHciService>(kEpAddr));
  EndpointHandler event_handler;

  static constexpr size_t kDataSize = 14;
  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kDataSize)))
        .offset(0)
        .size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());
  }
  // Ensure that QueueRequests has finished.
  ep_client->GetInfo();

  auto result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result->status, ZX_OK);

  event_handler.ExpectOnCompletion([](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
    ASSERT_EQ(event.completion().size(), 1UL);
    ASSERT_EQ(event.completion()[0].status(), ZX_ERR_IO_NOT_PRESENT);
    ASSERT_EQ(event.completion()[0].transfer_size(), 0);
  });
  ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
}

TEST_F(UsbVirtualBusTest, UnexpectedDisconnectDuringDeviceNormalTest) {
  EnableAndConnect();

  const uint8_t kEpAddr = 2 | USB_DIR_IN;
  fidl::SyncClient<fendpoint::Endpoint> ep_client(ConnectToEndpoint<fdci::UsbDciService>(kEpAddr));
  EndpointHandler event_handler;

  static constexpr size_t kDataSize = 14;
  {
    std::vector<frequest::Request> requests;
    std::vector<frequest::BufferRegion> buffer;
    buffer.emplace_back()
        .buffer(frequest::Buffer::WithData(std::vector<uint8_t>(kDataSize)))
        .offset(0)
        .size(kDataSize);
    requests.emplace_back(
        std::move(frequest::Request()
                      .information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo()))
                      .data(std::move(buffer))));

    ASSERT_TRUE(ep_client->QueueRequests(std::move(requests)).is_ok());
  }
  // Ensure that QueueRequests has finished.
  ep_client->GetInfo();

  auto result = virtual_bus().sync()->Disconnect();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result->status, ZX_OK);

  event_handler.ExpectOnCompletion([](fidl::Event<fendpoint::Endpoint::OnCompletion>& event) {
    ASSERT_EQ(event.completion().size(), 1UL);
    ASSERT_EQ(event.completion()[0].status(), ZX_ERR_IO_NOT_PRESENT);
    ASSERT_EQ(event.completion()[0].transfer_size(), 0);
  });
  ASSERT_TRUE(ep_client.HandleOneEvent(event_handler).ok());
}

TEST_F(UsbVirtualBusTest, GetHardwareInfo) {
  Enable();
  auto dci_client = driver_test().Connect<fdci::UsbDciService::Device>();
  ASSERT_TRUE(dci_client.is_ok());
  fidl::SyncClient dci(std::move(*dci_client));

  auto result = dci->GetHardwareInfo();
  ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();

  auto& info = result->info();
  ASSERT_TRUE(info.endpoints().has_value());
  // 15 OUT + 15 IN = 30 endpoints
  ASSERT_EQ(info.endpoints()->size(), 30u);
  EXPECT_FALSE(info.supports_dynamic_ep_sizing().value_or(true));

  // Verify first OUT endpoint (0x01)
  EXPECT_EQ(info.endpoints()->at(0).ep_address(), 0x01);
  ASSERT_TRUE(info.endpoints()->at(0).supported_types().has_value());
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->size(), 3u);
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->at(0).endpoint_type(),
            fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->at(0).max_packet_size_limit(), 65535u);

  // Verify first IN endpoint (0x81) at index 15
  EXPECT_EQ(info.endpoints()->at(15).ep_address(), 0x81);
  ASSERT_TRUE(info.endpoints()->at(15).supported_types().has_value());
  EXPECT_EQ(info.endpoints()->at(15).supported_types()->size(), 3u);
  EXPECT_EQ(info.endpoints()->at(15).supported_types()->at(0).endpoint_type(),
            fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  EXPECT_EQ(info.endpoints()->at(15).supported_types()->at(0).max_packet_size_limit(), 65535u);
}

}  // namespace
}  // namespace usb_virtual_bus
