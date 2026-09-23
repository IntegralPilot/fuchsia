// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-cdc-function.h"

#include <endian.h>
#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire_test_base.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fit/defer.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>

#include <atomic>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <usb-inspect/usb-inspect-test-helper.h>

#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"
#include "src/lib/testing/predicates/status.h"

namespace usb_cdc_function {

// UsbCdcTestHelper exposes hooks to inspect private endpoint state for tests.
// Note: In this test-only precursor CL, these methods return stubbed values to allow
// compilation against the unmodified driver. A subsequent CL introducing the
// dead-endpoint tracking feature will connect these methods to the real driver state.
class UsbCdcTestHelper {
 public:
  [[maybe_unused]] static bool IsIntrEndpointDead(const UsbCdcFunction& /*driver*/) {
    return false;
  }
  [[maybe_unused]] static bool IsRxEndpointDead(const UsbCdcFunction& /*driver*/) { return false; }
  [[maybe_unused]] static bool IsTxEndpointDead(const UsbCdcFunction& /*driver*/) { return false; }
  [[maybe_unused]] static size_t ActiveRxVmoIdsSize(const UsbCdcFunction& /*driver*/) { return 0; }
};

namespace {

constexpr uint8_t kBulkOutEp = 1;
constexpr uint8_t kBulkInEp = 2;
constexpr uint8_t kIntrEp = 3;
constexpr uint8_t kCommInterface = 0;
constexpr uint8_t kDataInterface = 1;

class FakeNetworkDeviceIfc : public fidl::testing::WireTestBase<fnetdev::NetworkDeviceIfc> {
 public:
  FakeNetworkDeviceIfc() = default;

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "FakeNetworkDeviceIfc not implemented: " << name;
    if (completer.is_reply_needed()) {
      completer.Close(ZX_ERR_NOT_SUPPORTED);
    }
  }

  void AddPort(fnetdev::wire::NetworkDeviceIfcAddPortRequest* request, fdf::Arena& arena,
               AddPortCompleter::Sync& completer) override {
    fit::function<void()> callback;
    {
      std::lock_guard<std::mutex> guard(lock_);
      port_id_ = request->id;
      port_ = std::move(request->port);
      callback = on_add_port_.share();
    }
    completer.ToAsync().buffer(arena).Reply(ZX_OK);
    if (callback) {
      callback();
    }
  }

  void CompleteRx(fnetdev::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                  CompleteRxCompleter::Sync& completer) override {
    fit::function<void()> callback;
    {
      std::lock_guard<std::mutex> guard(lock_);
      for (const auto& result : request->rx) {
        completed_rx_.push(fidl::ToNatural(result));
      }
      callback = on_complete_rx_.share();
    }
    if (callback) {
      callback();
    }
  }

  void CompleteTx(fnetdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                  CompleteTxCompleter::Sync& completer) override {
    fit::function<void()> callback;
    {
      std::lock_guard<std::mutex> guard(lock_);
      for (const auto& result : request->tx) {
        completed_tx_.push(result);
      }
      callback = on_complete_tx_.share();
    }
    if (callback) {
      callback();
    }
  }

  void PortStatusChanged(fnetdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                         fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) override {}

  bool HasPort() {
    std::lock_guard<std::mutex> guard(lock_);
    return port_.is_valid();
  }

  fdf::ClientEnd<fnetdev::NetworkPort> TakePort() {
    std::lock_guard<std::mutex> guard(lock_);
    return std::move(port_);
  }

  void set_on_add_port(fit::function<void()> callback) {
    std::lock_guard<std::mutex> guard(lock_);
    on_add_port_ = std::move(callback);
  }
  void set_on_complete_tx(fit::function<void()> callback) {
    std::lock_guard<std::mutex> guard(lock_);
    on_complete_tx_ = std::move(callback);
  }
  void set_on_complete_rx(fit::function<void()> callback) {
    std::lock_guard<std::mutex> guard(lock_);
    on_complete_rx_ = std::move(callback);
  }

  std::optional<fnetdev::wire::TxResult> PopCompleteTx() {
    std::lock_guard<std::mutex> guard(lock_);
    if (completed_tx_.empty()) {
      return std::nullopt;
    }
    auto tx = std::move(completed_tx_.front());
    completed_tx_.pop();
    return tx;
  }
  std::optional<fnetdev::RxBuffer> PopCompleteRx() {
    std::lock_guard<std::mutex> guard(lock_);
    if (completed_rx_.empty()) {
      return std::nullopt;
    }
    auto rx = std::move(completed_rx_.front());
    completed_rx_.pop();
    return rx;
  }

 private:
  std::mutex lock_;
  uint8_t port_id_;
  fdf::ClientEnd<fnetdev::NetworkPort> port_;
  fit::function<void()> on_add_port_;
  fit::function<void()> on_complete_tx_;
  fit::function<void()> on_complete_rx_;
  std::queue<fnetdev::wire::TxResult> completed_tx_;
  std::queue<fnetdev::RxBuffer> completed_rx_;
};

class DelayedCancelEndpoint : public fake_usb_endpoint::FakeEndpoint {
 public:
  ~DelayedCancelEndpoint() override {
    std::optional<CancelAllCompleter::Async> completer;
    {
      std::lock_guard<std::mutex> guard(lock_);
      completer = std::exchange(delayed_cancel_, std::nullopt);
    }
    if (completer.has_value()) {
      completer->Reply(fit::ok());
    }
  }

  void CancelAll(CancelAllCompleter::Sync& completer) override {
    {
      std::lock_guard<std::mutex> guard(lock_);
      cancel_all_called_ = true;
      if (hold_cancel_) {
        EXPECT_FALSE(delayed_cancel_.has_value())
            << "Concurrent CancelAll called while still held!";
        delayed_cancel_ = completer.ToAsync();
        return;
      }
    }
    DoCancelAll(completer.ToAsync());
  }

  void ReleaseCancelAll() {
    std::optional<CancelAllCompleter::Async> completer;
    {
      std::lock_guard<std::mutex> guard(lock_);
      completer = std::exchange(delayed_cancel_, std::nullopt);
    }
    if (completer.has_value()) {
      DoCancelAll(std::move(*completer));
    }
  }

  void set_hold_cancel(bool hold) {
    std::lock_guard<std::mutex> guard(lock_);
    hold_cancel_ = hold;
  }
  bool has_delayed_cancel() {
    std::lock_guard<std::mutex> guard(lock_);
    return delayed_cancel_.has_value();
  }

  bool has_pending_requests() {
    std::lock_guard<std::mutex> guard(lock_);
    return !requests_.empty();
  }

  bool cancel_all_called() {
    std::lock_guard<std::mutex> guard(lock_);
    return cancel_all_called_;
  }
  void reset_cancel_all_called() {
    std::lock_guard<std::mutex> guard(lock_);
    cancel_all_called_ = false;
  }

  bool enabled() {
    std::lock_guard<std::mutex> guard(lock_);
    return enabled_;
  }

  // Unbinds the active FIDL server binding for the endpoint, simulating an abrupt
  // transport closure or peer disconnection.
  void Close() {
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    {
      std::lock_guard<std::mutex> guard(lock_);
      local_binding = binding_ref_;
      binding_ref_.reset();
    }
    if (local_binding) {
      local_binding->Unbind();
    }
  }

 private:
  void DoCancelAll(CancelAllCompleter::Async completer) {
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    {
      std::lock_guard<std::mutex> guard(lock_);
      completions = CancelAllLocked(/*is_disable=*/false);
      local_binding = binding_ref_;
    }
    if (!completions.empty() && local_binding) {
      (void)fidl::SendEvent(*local_binding)->OnCompletion(std::move(completions));
    }
    completer.Reply(fit::ok());
  }

  bool hold_cancel_ = false;
  bool cancel_all_called_ = false;
  std::optional<CancelAllCompleter::Async> delayed_cancel_;
};

class FakeUsbFunction
    : public fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction,
                                                    DelayedCancelEndpoint> {
 public:
  using Base = fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction,
                                                      DelayedCancelEndpoint>;
  using Base::Base;

  ~FakeUsbFunction() override {
    std::optional<fidl::internal::NaturalCompleter<
        fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Async>
        completer;
    {
      std::lock_guard<std::mutex> guard(lock_);
      completer = std::exchange(delayed_deconfigure_completer_, std::nullopt);
    }
    if (completer.has_value()) {
      completer->Reply(fit::ok());
    }
  }

  void Configure(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure>& request,
      fidl::internal::NaturalCompleter<fuchsia_hardware_usb_function::UsbFunction::Configure>::Sync&
          completer) override {
    fit::callback<void()> callback;
    {
      std::lock_guard<std::mutex> guard(lock_);
      interface_ = std::move(request.iface());
      callback = std::move(on_configure_);
    }
    completer.Reply(fit::ok());
    if (callback) {
      std::move(callback)();
    }
  }

  void Deconfigure(
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Sync& completer) override {
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (hold_deconfigure_) {
        EXPECT_FALSE(delayed_deconfigure_completer_.has_value())
            << "Concurrent Deconfigure detected while still held!";
        delayed_deconfigure_completer_ = completer.ToAsync();
        return;
      }
    }
    completer.Reply(fit::ok());
  }

  void ReleaseDelayedDeconfigure() {
    std::optional<fidl::internal::NaturalCompleter<
        fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Async>
        completer;
    {
      std::lock_guard<std::mutex> guard(lock_);
      completer = std::exchange(delayed_deconfigure_completer_, std::nullopt);
    }
    if (completer.has_value()) {
      completer->Reply(fit::ok());
    }
  }

  bool has_delayed_deconfigure() {
    std::lock_guard<std::mutex> guard(lock_);
    return delayed_deconfigure_completer_.has_value();
  }
  void set_hold_deconfigure(bool hold) {
    std::lock_guard<std::mutex> guard(lock_);
    hold_deconfigure_ = hold;
  }

  void AllocResources(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::AllocResources>::Sync& completer) override {
    fuchsia_hardware_usb_function::UsbFunctionAllocResourcesResponse response;
    ASSERT_EQ(request.endpoints().size(), 3u);
    ASSERT_EQ(request.interface_count(), 2u);
    ASSERT_EQ(request.strings().size(), 1u);
    response.interface_nums() = {kCommInterface, kDataInterface};
    response.endpoint_addrs() = {kIntrEp, kBulkInEp, kBulkOutEp};
    response.string_indices() = {1};
    for (size_t i = 0; i < 3; i++) {
      fidl::ServerEnd ep = std::move(request.endpoints()[i].endpoint());
      fake_endpoint(response.endpoint_addrs()[i]).Connect(dispatcher(), std::move(ep));
    }
    completer.Reply(fit::ok(std::move(response)));
  }

  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> TakeInterface() {
    std::lock_guard<std::mutex> guard(lock_);
    return std::move(interface_);
  }

  void DisableEndpoint(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>::Sync& completer) override {
    std::optional<zx_status_t> error;
    bool verify = false;
    {
      std::lock_guard<std::mutex> guard(lock_);
      error = disable_endpoint_error_;
      verify = verify_lifecycle_order_;
    }

    if (error.has_value()) {
      completer.Reply(fit::error(error.value()));
      return;
    }

    uint8_t ep_addr = request.endpoint_address();
    auto& fake_ep = fake_endpoint(ep_addr);

    if (verify) {
      // Verify lifecycle ordering.
      EXPECT_FALSE(fake_ep.has_pending_requests())
          << "DisableEndpoint called while requests are still pending on endpoint!";
      EXPECT_FALSE(fake_ep.has_delayed_cancel())
          << "DisableEndpoint called while CancelAll is still held!";
    }

    fake_ep.DisableAndCancelAll();
    completer.Reply(fit::ok());
  }

  void set_verify_lifecycle_order(bool verify) {
    std::lock_guard<std::mutex> guard(lock_);
    verify_lifecycle_order_ = verify;
  }
  void set_disable_endpoint_error(std::optional<zx_status_t> error) {
    std::lock_guard<std::mutex> guard(lock_);
    disable_endpoint_error_ = error;
  }

  void set_on_configure(fit::callback<void()> callback) {
    std::lock_guard<std::mutex> guard(lock_);
    on_configure_ = std::move(callback);
  }

 private:
  std::mutex lock_;
  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> interface_;
  fit::callback<void()> on_configure_;
  bool verify_lifecycle_order_ = false;
  bool hold_deconfigure_ = false;
  std::optional<fidl::internal::NaturalCompleter<
      fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Async>
      delayed_deconfigure_completer_;
  std::optional<zx_status_t> disable_endpoint_error_;
};

class Environment : public fdf_testing::Environment {
 public:
  void CloseUsbFunctionServer() { usb_function_bindings_.CloseAll(ZX_ERR_PEER_CLOSED); }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    device_server_.Initialize("default", std::nullopt);
    zx_status_t status = device_server_.Serve(dispatcher, &to_driver_vfs);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    fuchsia_hardware_usb_function::UsbFunctionService::InstanceHandler handler({
        .device = usb_function_bindings_.CreateHandler(&fake_usb_fidl_, dispatcher,
                                                       fidl::kIgnoreBindingClosure),
    });

    if (mac_address_.has_value()) {
      if (zx::result result =
              metadata_server_.Serve(to_driver_vfs, dispatcher, mac_address_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (zx::result result =
            to_driver_vfs.AddService<fuchsia_hardware_usb_function::UsbFunctionService>(
                std::move(handler));
        result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  using FakeUsbFidl = FakeUsbFunction;

  compat::DeviceServer device_server_;
  FakeUsbFunction fake_usb_fidl_{fdf::Dispatcher::GetCurrent()->async_dispatcher()};
  fidl::ServerBindingGroup<fuchsia_hardware_usb_function::UsbFunction> usb_function_bindings_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata> metadata_server_;
  FakeNetworkDeviceIfc fake_ifc_;
  std::optional<fuchsia_boot_metadata::MacAddressMetadata> mac_address_;
};

class UsbCdcTestConfig final {
 public:
  using DriverType = UsbCdcFunction;
  using EnvironmentType = Environment;
};

class UsbCdcTest : public ::testing::Test {
 public:
  static constexpr fdf_arena_tag_t kArenaTag = 'TEST';
  static constexpr std::array<uint8_t, 6> kTestMac = {0, 1, 2, 3, 4, 5};

  virtual void ConfigureMetadata(Environment& env) {
    env.mac_address_ =
        fuchsia_boot_metadata::MacAddressMetadata{{.mac_address = {{{.octets = kTestMac}}}}};
  }

  void SetUp() override {
    auto endpoints = fdf::CreateEndpoints<fnetdev::NetworkDeviceIfc>();
    auto port_ready = std::make_shared<libsync::Completion>();
    auto function_configured = std::make_shared<libsync::Completion>();
    driver_test_.RunInEnvironmentTypeContext([this, server = std::move(endpoints->server),
                                              port_ready,
                                              function_configured](Environment& env) mutable {
      ConfigureMetadata(env);
      fdf::BindServer(fdf::Dispatcher::GetCurrent()->get(), std::move(server), &env.fake_ifc_);
      env.fake_ifc_.set_on_add_port([port_ready]() { port_ready->Signal(); });
      env.fake_usb_fidl_.set_on_configure(
          [function_configured]() { function_configured->Signal(); });
    });

    auto start_result = driver_test_.StartDriver();
    if (start_result.is_error()) {
      driver_stopped_ = true;
    }
    if (!expect_start_success_) {
      ASSERT_TRUE(start_result.is_error());
      return;
    }
    ASSERT_OK(start_result.status_value());

    // Connect to the driver
    auto connect_result = driver_test_.Connect<fnetdev::Service::NetworkDeviceImpl>();
    ASSERT_OK(connect_result.status_value());

    fdf::Arena arena(kArenaTag);
    net_impl_client_.Bind(std::move(connect_result.value()));
    auto init_result = net_impl_client_.buffer(arena)->Init(std::move(endpoints->client));
    ASSERT_OK(init_result.status());
    ASSERT_OK(init_result->s);

    ASSERT_OK(port_ready->Wait(zx::deadline_after(zx::sec(5))));
    ASSERT_OK(function_configured->Wait(zx::deadline_after(zx::sec(5))));

    driver_test_.RunInEnvironmentTypeContext([this](Environment& env) {
      EXPECT_TRUE(env.fake_ifc_.HasPort());
      net_port_client_.Bind(env.fake_ifc_.TakePort());
      function_client_.Bind(env.fake_usb_fidl_.TakeInterface());
    });
    driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {
      EXPECT_EQ(driver.InterruptAddress(), kIntrEp);
      EXPECT_EQ(driver.BulkInAddress(), kBulkInEp);
      EXPECT_EQ(driver.BulkOutAddress(), kBulkOutEp);
    });
  }

  void TearDown() override {
    driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
      env.fake_ifc_.set_on_complete_rx(nullptr);
      env.fake_ifc_.set_on_complete_tx(nullptr);

      // Deterministically reset persistent mock holds to prevent cross-test leakage!
      for (uint8_t ep : {kBulkOutEp, kIntrEp, kBulkInEp}) {
        auto& fake_ep = env.fake_usb_fidl_.fake_endpoint(ep);
        fake_ep.set_hold_cancel(false);
        fake_ep.ReleaseCancelAll();
        fake_ep.reset_cancel_all_called();
      }
      env.fake_usb_fidl_.set_hold_deconfigure(false);
      env.fake_usb_fidl_.set_disable_endpoint_error(std::nullopt);
      env.fake_usb_fidl_.set_verify_lifecycle_order(false);
      env.fake_usb_fidl_.ReleaseDelayedDeconfigure();
    });
    if (!driver_stopped_) {
      ASSERT_OK(driver_test_.StopDriver().status_value());
      driver_stopped_ = true;
    }
  }

  static constexpr uint8_t kVmoId = 1;

  void StartNetworkDevice() {
    fdf::Arena arena(kArenaTag);
    auto start_result = net_impl_client_.buffer(arena)->Start();
    ASSERT_OK(start_result.status());
    ASSERT_OK(start_result->s);
  }

  void ExecuteMockNetworkTransaction(uint32_t buffer_id, size_t length, zx_status_t expected_status,
                                     fit::function<void()> middle_action = nullptr) {
    fdf::Arena arena(kArenaTag);
    auto tx_completed = std::make_shared<libsync::Completion>();
    driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
      env.fake_ifc_.set_on_complete_tx([tx_completed]() { tx_completed->Signal(); });
    });
    auto cleanup = fit::defer([&]() {
      driver_test_.RunInEnvironmentTypeContext(
          [&](Environment& env) { env.fake_ifc_.set_on_complete_tx(nullptr); });
    });

    fnetdev::wire::BufferRegion region = {
        .vmo = kVmoId,
        .offset = 0,
        .length = length,
    };
    fnetdev::wire::TxBuffer tx_buffer{
        .id = buffer_id,
        .data = fidl::VectorView<fnetdev::wire::BufferRegion>::FromExternal(&region, 1),
    };
    ASSERT_OK(net_impl_client_.buffer(arena)
                  ->QueueTx(fidl::VectorView<fnetdev::wire::TxBuffer>::FromExternal(&tx_buffer, 1))
                  .status());

    if (middle_action) {
      EXPECT_STATUS(tx_completed->Wait(zx::time::infinite_past()), ZX_ERR_TIMED_OUT);
      middle_action();
    }

    ASSERT_OK(tx_completed->Wait(zx::deadline_after(zx::sec(5))));
    driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
      std::optional complete = env.fake_ifc_.PopCompleteTx();
      ASSERT_TRUE(complete.has_value());
      EXPECT_EQ(complete->id, buffer_id);
      EXPECT_EQ(complete->status, expected_status);
    });
  }

  void SetConfiguredAndEnable() {
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
          .interface = kDataInterface,
          .alt_setting = 1,
      }});
      ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
    }
  }

 protected:
  fdf_testing::BackgroundDriverTest<UsbCdcTestConfig> driver_test_;
  fdf::WireSyncClient<fnetdev::NetworkDeviceImpl> net_impl_client_;
  fdf::WireSyncClient<fnetdev::NetworkPort> net_port_client_;
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunctionInterface> function_client_;
  bool driver_stopped_ = false;
  bool expect_start_success_ = true;
};

// Validates that GetInfo returns the expected TX and RX buffer queue depths for the driver.
TEST_F(UsbCdcTest, GetInfo) {
  fdf::Arena arena(kArenaTag);
  auto result = net_impl_client_.buffer(arena)->GetInfo();
  ASSERT_OK(result.status());
  EXPECT_EQ(result->info.tx_depth(), UsbCdcFunction::kTxDepth);
  EXPECT_EQ(result->info.rx_depth(), UsbCdcFunction::kRxDepth);
}

// Validates that GetMac retrieves the provisioned Ethernet MAC address from boot metadata.
TEST_F(UsbCdcTest, GetMac) {
  StartNetworkDevice();

  fdf::Arena arena(kArenaTag);
  auto result = net_port_client_.buffer(arena)->GetMac();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->mac_ifc.is_valid());

  fdf::WireSyncClient<fnetdev::MacAddr> mac_client;
  mac_client.Bind(std::move(result->mac_ifc));

  auto mac_result = mac_client.buffer(arena)->GetAddress();
  ASSERT_OK(mac_result.status());

  // TODO(https://fxbug.dev/476474119): The driver is currently flipping MAC
  // addresses, we should always use the local mac address provided by the
  // metadata and offer the other one to the host.
  std::array<uint8_t, 6> expected_mac = kTestMac;
  expected_mac[5] ^= 0x01;
  EXPECT_THAT(mac_result->mac.octets, testing::ElementsAreArray(expected_mac));
}

// Validates that transmit packet requests are immediately rejected when the driver is offline.
TEST_F(UsbCdcTest, TxFailsIfOffline) {
  StartNetworkDevice();

  constexpr uint32_t kBufferId = 100;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  uint8_t data[] = {0xAA, 0xBB, 0xCC, 0xDD};
  ASSERT_OK(vmo.write(data, 0, sizeof(data)));
  fdf::Arena arena(kArenaTag);
  auto prepare_result = net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare_result.status());
  ASSERT_OK(prepare_result->s);

  ASSERT_NO_FATAL_FAILURE(ExecuteMockNetworkTransaction(kBufferId, sizeof(data), ZX_ERR_BAD_STATE));
}

// Validates end-to-end transmit data packet transfer from network device to the USB bulk IN
// endpoint.
TEST_F(UsbCdcTest, QueueTx) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  constexpr uint32_t kBufferId = 100;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  uint8_t data[] = {0xAA, 0xBB, 0xCC, 0xDD};
  ASSERT_OK(vmo.write(data, 0, sizeof(data)));
  fdf::Arena arena(kArenaTag);
  auto prepare_result = net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare_result.status());
  ASSERT_OK(prepare_result->s);

  auto trigger_usb = [&]() {
    driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
      env.fake_usb_fidl_.fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, sizeof(data));
    });
  };
  ASSERT_NO_FATAL_FAILURE(
      ExecuteMockNetworkTransaction(kBufferId, sizeof(data), ZX_OK, trigger_usb));
}

// Validates end-to-end receive data packet transfer from the USB bulk OUT endpoint to network
// buffers.
TEST_F(UsbCdcTest, Rx) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  auto rx_completed = std::make_shared<libsync::Completion>();
  driver_test_.RunInEnvironmentTypeContext([rx_completed](Environment& env) {
    env.fake_ifc_.set_on_complete_rx([rx_completed]() { rx_completed->Signal(); });
  });

  constexpr size_t kDataSize1 = 54;
  constexpr size_t kDataSize2 = 250;
  constexpr uint8_t kBufferId1 = 201;
  constexpr uint8_t kBufferId2 = 202;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  fdf::Arena arena(kArenaTag);
  auto prepare_result = net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare_result.status());
  ASSERT_OK(prepare_result->s);

  {
    fnetdev::wire::RxSpaceBuffer buffer = {
        .id = kBufferId1,
        .region = {.vmo = kVmoId, .offset = 0, .length = 2048},
    };

    ASSERT_OK(
        net_impl_client_.buffer(arena)
            ->QueueRxSpace(fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&buffer, 1))
            .status());
  }

  EXPECT_STATUS(rx_completed->Wait(zx::time::infinite_past()), ZX_ERR_TIMED_OUT);

  // Complete 2 requests, but we only have one rx space already queued.
  // The pending request can be completed later.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, kDataSize1);
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, kDataSize2);
  });

  ASSERT_OK(rx_completed->Wait(zx::deadline_after(zx::sec(5))));

  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto buffer = env.fake_ifc_.PopCompleteRx();
    ASSERT_TRUE(buffer.has_value());
    EXPECT_EQ(buffer->meta().port(), UsbCdcFunction::kPortId);
    EXPECT_EQ(buffer->meta().frame_type(), fuchsia_hardware_network::wire::FrameType::kEthernet);
    ASSERT_EQ(buffer->data().size(), 1u);
    auto& data = buffer->data()[0];
    EXPECT_EQ(data.offset(), 0u);
    EXPECT_EQ(data.length(), kDataSize1);
    EXPECT_EQ(data.id(), kBufferId1);
  });

  rx_completed->Reset();

  {
    fnetdev::wire::RxSpaceBuffer buffer = {
        .id = kBufferId2,
        .region = {.vmo = kVmoId, .offset = 0, .length = 2048},
    };

    ASSERT_OK(
        net_impl_client_.buffer(arena)
            ->QueueRxSpace(fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&buffer, 1))
            .status());
  }

  ASSERT_OK(rx_completed->Wait(zx::deadline_after(zx::sec(5))));

  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto buffer = env.fake_ifc_.PopCompleteRx();
    ASSERT_TRUE(buffer.has_value());
    EXPECT_EQ(buffer->meta().port(), UsbCdcFunction::kPortId);
    EXPECT_EQ(buffer->meta().frame_type(), fuchsia_hardware_network::wire::FrameType::kEthernet);
    ASSERT_EQ(buffer->data().size(), 1u);
    auto& data = buffer->data()[0];
    EXPECT_EQ(data.offset(), 0u);
    EXPECT_EQ(data.length(), kDataSize2);
    EXPECT_EQ(data.id(), kBufferId2);
  });
}

// Validates that stopping the network device drains and cancels all active in-flight transactions.
TEST_F(UsbCdcTest, Stop) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  constexpr uint8_t kRxBufferId = 2;
  constexpr uint8_t kTxBufferId = 3;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  fdf::Arena arena(kArenaTag);
  auto prepare_result = net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare_result.status());
  ASSERT_OK(prepare_result->s);

  fnetdev::wire::RxSpaceBuffer rx_buffer = {
      .id = kRxBufferId,
      .region = {.vmo = kVmoId, .offset = 0, .length = 2048},
  };

  ASSERT_OK(net_impl_client_.buffer(arena)
                ->QueueRxSpace(
                    fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&rx_buffer, 1))
                .status());

  auto rx_completed = std::make_shared<libsync::Completion>();
  driver_test_.RunInEnvironmentTypeContext([rx_completed](Environment& env) {
    env.fake_ifc_.set_on_complete_rx([rx_completed]() { rx_completed->Signal(); });
  });

  auto trigger_stop = [&]() {
    auto result = net_impl_client_.buffer(arena)->Stop();
    ASSERT_OK(result.status());
  };
  ASSERT_NO_FATAL_FAILURE(
      ExecuteMockNetworkTransaction(kTxBufferId, 2048, ZX_ERR_CANCELED, trigger_stop));

  ASSERT_OK(rx_completed->Wait(zx::deadline_after(zx::sec(5))));

  // All in flight transactions should complete on stop.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto rx = env.fake_ifc_.PopCompleteRx();
    ASSERT_TRUE(rx.has_value());
    ASSERT_EQ(rx->data().size(), 1u);
    EXPECT_EQ(rx->data()[0].id(), kRxBufferId);
  });
}

// Validates that driver teardown cleanly handles buffered RX completions without hanging or
// leaking.
TEST_F(UsbCdcTest, TeardownWithPendingRxCompletion) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, 123);
  });

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInDriverContext(
            [&ready](UsbCdcFunction& driver) { ready = driver.HasPendingRxCompletions(); });
        return ready;
      },
      zx::sec(5)));

  // Bulk of verification happens on test teardown as part of stopping the
  // driver.
}

// Validates the strict asynchronous teardown ordering: requests are cancelled on all active
// endpoints and returned before deconfiguring the USB function or disabling endpoints. Requires
// asynchronous SetConfigured(false) and Stop() teardown coordinator in the driver.
TEST_F(UsbCdcTest, VerifySafeTeardownSequence) {
  // 1. Bring the network device online and configure alternate settings to queue requests on
  // endpoints.
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // 2. Configure mock bulk and interrupt endpoints to hold CancelAll completions!
  // Note: kBulkInEp is not held because it starts with zero queued requests;
  // allowing its CancelAll call to complete immediately in the mock avoids blocking.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(true);
    env.fake_usb_fidl_.fake_endpoint(kIntrEp).set_hold_cancel(true);
    env.fake_usb_fidl_.set_hold_deconfigure(true);
    env.fake_usb_fidl_.set_verify_lifecycle_order(true);
  });

  // 3. Retrieve the environment's dispatcher and pointer so we can run our coordinator on it
  // safely.
  async_dispatcher_t* env_dispatcher = nullptr;
  Environment* env_ptr = nullptr;
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env_dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    env_ptr = &env;
  });
  ASSERT_NE(env_dispatcher, nullptr);
  ASSERT_NE(env_ptr, nullptr);

  // 4. Define our non-blocking step-by-step validation coordinator.
  // This runs entirely on the environment's dispatcher thread, allowing it to execute
  // concurrently while the main thread is blocked inside StopDriver().
  // We wrap the recursive function inside a std::shared_ptr to ensure memory-safe
  // capture by value in async delayed tasks, avoiding dangling stack references.
  auto test_active = std::make_shared<std::atomic<bool>>(true);
  auto poll_and_verify = std::make_shared<fit::function<void(int)>>();
  std::weak_ptr<fit::function<void(int)>> weak_poll = poll_and_verify;
  auto teardown_attempts = std::make_shared<int>(0);
  *poll_and_verify = [env_ptr, env_dispatcher, weak_poll, teardown_attempts,
                      test_active](int step) {
    if (!test_active->load()) {
      return;
    }
    auto locked_poll = weak_poll.lock();
    if (!locked_poll) {
      return;
    }

    if (++(*teardown_attempts) > 5000) {
      ADD_FAILURE() << "Timed out waiting for safe teardown sequence step " << step;
      env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(false);
      env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).set_hold_cancel(false);
      env_ptr->fake_usb_fidl_.set_hold_deconfigure(false);
      if (env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).has_delayed_cancel()) {
        env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).ReleaseCancelAll();
      }
      if (env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).has_delayed_cancel()) {
        env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).ReleaseCancelAll();
      }
      if (env_ptr->fake_usb_fidl_.has_delayed_deconfigure()) {
        env_ptr->fake_usb_fidl_.ReleaseDelayedDeconfigure();
      }
      return;
    }

    switch (step) {
      case 0: {
        // Wait for active CancelAll requests (OUT and INTR) to be delayed and held.
        bool captured_out = env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).has_delayed_cancel();
        bool captured_intr = env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).has_delayed_cancel();

        if (!captured_out || !captured_intr) {
          // If active endpoint cancellations have not been received, poll again in 1ms
          async::PostDelayedTask(
              env_dispatcher, [step, locked_poll]() { (*locked_poll)(step); }, zx::msec(1));
          return;
        }

        // Verify endpoints are still enabled and Deconfigure is NOT called while completions are
        // held.
        EXPECT_TRUE(env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).enabled());
        EXPECT_TRUE(env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).enabled());
        EXPECT_FALSE(env_ptr->fake_usb_fidl_.has_delayed_deconfigure());

        // Release CancelAll on bulk OUT endpoint, verify Deconfigure is still NOT called!
        env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(false);
        env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).ReleaseCancelAll();
        async::PostDelayedTask(env_dispatcher, [locked_poll]() { (*locked_poll)(1); }, zx::msec(5));
        break;
      }
      case 1: {
        EXPECT_FALSE(env_ptr->fake_usb_fidl_.has_delayed_deconfigure());
        EXPECT_TRUE(env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).enabled());
        // Release CancelAll on interrupt endpoint, wait for Deconfigure to be called!
        env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).set_hold_cancel(false);
        env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).ReleaseCancelAll();
        async::PostDelayedTask(env_dispatcher, [locked_poll]() { (*locked_poll)(2); }, zx::msec(5));
        break;
      }
      case 2: {
        bool deconfigured = env_ptr->fake_usb_fidl_.has_delayed_deconfigure();
        if (!deconfigured) {
          async::PostDelayedTask(
              env_dispatcher, [step, locked_poll]() { (*locked_poll)(step); }, zx::msec(1));
          return;
        }

        // Release the Deconfigure completer to finish teardown cleanly.
        env_ptr->fake_usb_fidl_.ReleaseDelayedDeconfigure();
        break;
      }
      default:
        ADD_FAILURE() << "Unexpected step in safe teardown sequence: " << step;
        break;
    }
  };

  // 5. Post the coordinator task onto the environment dispatcher thread before calling StopDriver.
  async::PostTask(env_dispatcher, [poll_and_verify]() { (*poll_and_verify)(0); });

  // 6. Call StopDriver() synchronously on the main thread.
  // It will block the main thread, but the environment's dispatcher thread will run
  // our validation tasks and successfully unblock the stop sequence!
  ASSERT_OK(driver_test_.StopDriver().status_value());
  driver_stopped_ = true;
  test_active->store(false);
}

// Validates that transitioning to alternate setting 0 (deconfigured/idle) returns all queued RX
// space buffers to the network device. Requires driver support to drain rx_space_buffers_ on
// unconfigure.
TEST_F(UsbCdcTest, UnconfigureReturnsRxSpace) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  constexpr uint8_t kRxBufferId = 2;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  fdf::Arena arena(kArenaTag);
  auto prepare_result = net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare_result.status());
  ASSERT_OK(prepare_result->s);

  fnetdev::wire::RxSpaceBuffer rx_buffer = {
      .id = kRxBufferId,
      .region = {.vmo = kVmoId, .offset = 0, .length = 2048},
  };

  ASSERT_OK(net_impl_client_.buffer(arena)
                ->QueueRxSpace(
                    fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&rx_buffer, 1))
                .status());

  // Synchronize with the driver runtime to ensure QueueRxSpace has been fully processed.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {});

  auto rx_completed = std::make_shared<libsync::Completion>();
  driver_test_.RunInEnvironmentTypeContext([rx_completed](Environment& env) {
    env.fake_ifc_.set_on_complete_rx([rx_completed]() { rx_completed->Signal(); });
  });

  EXPECT_STATUS(rx_completed->Wait(zx::time::infinite_past()), ZX_ERR_TIMED_OUT);

  // Unconfigure USB. This should trigger immediate return of RX space.
  {
    ASSERT_TRUE(function_client_.is_valid());
    fidl::Result result = function_client_->SetConfigured({{
        .configured = false,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Wait for completion. In the buggy version, this will timeout because they are not returned.
  ASSERT_OK(rx_completed->Wait(zx::deadline_after(zx::sec(5))));

  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto rx = env.fake_ifc_.PopCompleteRx();
    ASSERT_TRUE(rx.has_value());
    ASSERT_EQ(rx->data().size(), 1u);
    EXPECT_EQ(rx->data()[0].id(), kRxBufferId);
    EXPECT_EQ(rx->data()[0].length(), 0u);
  });
}

// Validates that reconfiguring the device re-enables and configures the Interrupt IN endpoint
// across configuration sessions. Requires driver to reset completed endpoint tracking on
// re-configuration.
TEST_F(UsbCdcTest, ReconfigurationReenablesInterruptEndpoint) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Simulate a rapid host reset by deconfiguring.
  {
    ASSERT_TRUE(function_client_.is_valid());
    fidl::Result result = function_client_->SetConfigured({{
        .configured = false,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Re-configure the device (second configuration session).
  // This should successfully re-enable and configure the Interrupt IN endpoint
  // instead of bypassing!
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Verify that the driver is fully online and configured!
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.online()); });
}

// Validates that a host-initiated soft reset (SetConfigured(true) when already configured) safely
// drains previous session state and reconfigures endpoints without error.
TEST_F(UsbCdcTest, SoftResetReconfiguresEndpoints) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Host sends SetConfigured(true) while already configured (soft reset).
  ASSERT_TRUE(function_client_.is_valid());
  fidl::Result result = function_client_->SetConfigured({{
      .configured = true,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();

  // Driver should remain configured.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.configured()); });
}

// Validates that rapidly toggling SetConfigured(true) followed immediately by
// SetConfigured(false) leaves the driver in the unconfigured state as requested.
TEST_F(UsbCdcTest, RapidToggleConfigured) {
  StartNetworkDevice();
  ASSERT_TRUE(function_client_.is_valid());

  // Dispatch SetConfigured(true) followed immediately by SetConfigured(false).
  auto client_end = function_client_.TakeClientEnd();
  auto toggle_complete = std::make_shared<libsync::Completion>();
  std::shared_ptr<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      async_function_client;
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    async_function_client =
        std::make_shared<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>(
            std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());
    (*async_function_client)
        ->SetConfigured({{
            .configured = true,
            .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
        }})
        .Then([](auto& result) {});
    (*async_function_client)
        ->SetConfigured({{
            .configured = false,
            .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined,
        }})
        .Then([toggle_complete](auto& result) {
          EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
          toggle_complete->Signal();
        });
  });

  ASSERT_OK(toggle_complete->Wait(zx::deadline_after(zx::sec(5))));
  driver_test_.RunInDriverContext(
      [](UsbCdcFunction& driver) { EXPECT_FALSE(driver.configured()); });
  driver_test_.RunInEnvironmentTypeContext(
      [&](Environment& env) { async_function_client.reset(); });
}

// Validates that endpoints are not prematurely disabled while asynchronous cancellation is still in
// flight during driver Stop. Requires driver Stop() to await CancelAll completion before calling
// DisableEndpoint.
TEST_F(UsbCdcTest, TrapPrematureDisableOnStop) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(true);
    env.fake_usb_fidl_.fake_endpoint(kIntrEp).set_hold_cancel(true);
    env.fake_usb_fidl_.set_verify_lifecycle_order(true);
  });

  async_dispatcher_t* env_dispatcher = nullptr;
  Environment* env_ptr = nullptr;
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env_dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    env_ptr = &env;
  });
  ASSERT_NE(env_dispatcher, nullptr);
  ASSERT_NE(env_ptr, nullptr);

  auto test_active = std::make_shared<std::atomic<bool>>(true);
  auto poll_and_release = std::make_shared<fit::function<void()>>();
  std::weak_ptr<fit::function<void()>> weak_poll = poll_and_release;
  auto stop_attempts = std::make_shared<int>(0);
  *poll_and_release = [env_ptr, env_dispatcher, weak_poll, stop_attempts, test_active]() {
    if (!test_active->load()) {
      return;
    }
    auto locked_poll = weak_poll.lock();
    if (!locked_poll) {
      return;
    }
    if (++(*stop_attempts) > 5000) {
      ADD_FAILURE() << "Timed out waiting for endpoints to reach delayed cancel state";
      env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(false);
      env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).set_hold_cancel(false);
      if (env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).has_delayed_cancel()) {
        env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).ReleaseCancelAll();
      }
      if (env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).has_delayed_cancel()) {
        env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).ReleaseCancelAll();
      }
      return;
    }
    if (!env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).has_delayed_cancel() ||
        !env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).has_delayed_cancel()) {
      async::PostDelayedTask(env_dispatcher, [locked_poll]() { (*locked_poll)(); }, zx::msec(1));
      return;
    }
    // Verify endpoints have not been prematurely disabled while cancellations were pending.
    EXPECT_TRUE(env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).enabled());
    EXPECT_TRUE(env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).enabled());
    env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(false);
    env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).set_hold_cancel(false);
    env_ptr->fake_usb_fidl_.fake_endpoint(kBulkOutEp).ReleaseCancelAll();
    env_ptr->fake_usb_fidl_.fake_endpoint(kIntrEp).ReleaseCancelAll();
  };

  async::PostTask(env_dispatcher, [poll_and_release]() { (*poll_and_release)(); });

  // Call driver_test_.StopDriver().
  ASSERT_OK(driver_test_.StopDriver().status_value());
  driver_stopped_ = true;
  test_active->store(false);
}

// Validates that all active endpoints receive CancelAll during deconfiguration before releasing
// interface resources. Requires SetConfigured(false) to trigger CancelAll across all active
// endpoints.
TEST_F(UsbCdcTest, TrapMissingCancelOnDeconfigure) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Send an asynchronous FIDL request to SetConfigured({false}) on the environment dispatcher.
  ASSERT_TRUE(function_client_.is_valid());
  auto client_end = function_client_.TakeClientEnd();

  auto set_configured_complete = std::make_shared<libsync::Completion>();
  std::shared_ptr<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      async_function_client;
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    async_function_client =
        std::make_shared<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>(
            std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());
    (*async_function_client)
        ->SetConfigured({{
            .configured = false,
            .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined,
        }})
        .Then([set_configured_complete](auto& result) {
          EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
          set_configured_complete->Signal();
        });
  });

  // Use FDF RunWithTimeoutOrUntil to safely wait for deconfiguration cancels to complete.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool complete = false;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          complete = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).cancel_all_called() &&
                     env.fake_usb_fidl_.fake_endpoint(kIntrEp).cancel_all_called();
        });
        return complete;
      },
      zx::sec(5)));

  // Check the mock endpoints.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    EXPECT_FALSE(env.fake_usb_fidl_.fake_endpoint(kBulkInEp).cancel_all_called());
    EXPECT_TRUE(env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).cancel_all_called());
    EXPECT_TRUE(env.fake_usb_fidl_.fake_endpoint(kIntrEp).cancel_all_called());
  });

  ASSERT_OK(set_configured_complete->Wait(zx::deadline_after(zx::sec(5))));
  driver_test_.RunInEnvironmentTypeContext(
      [&](Environment& env) { async_function_client.reset(); });
}

// Validates that Inspect tracks online state, throughput metrics, and TX/RX byte counters.
TEST_F(UsbCdcTest, Inspect) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  constexpr uint32_t kTxBufferId = 100;
  constexpr uint8_t kRxBufferId = 201;
  constexpr size_t kTxDataSize = 4;
  constexpr size_t kRxDataSize = 54;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  uint8_t tx_data[] = {0xAA, 0xBB, 0xCC, 0xDD};
  ASSERT_OK(vmo.write(tx_data, 0, sizeof(tx_data)));

  fdf::Arena arena(kArenaTag);
  auto prepare_result = net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare_result.status());
  ASSERT_OK(prepare_result->s);

  // 1. Queue TX
  fnetdev::wire::BufferRegion tx_region = {.vmo = kVmoId, .offset = 0, .length = kTxDataSize};
  fnetdev::wire::TxBuffer tx_buffer = {
      .id = kTxBufferId,
      .data = fidl::VectorView<fnetdev::wire::BufferRegion>::FromExternal(&tx_region, 1),
  };

  auto tx_completed = std::make_shared<libsync::Completion>();
  driver_test_.RunInEnvironmentTypeContext([tx_completed](Environment& env) {
    env.fake_ifc_.set_on_complete_tx([tx_completed]() { tx_completed->Signal(); });
  });
  ASSERT_OK(net_impl_client_.buffer(arena)
                ->QueueTx(fidl::VectorView<fnetdev::wire::TxBuffer>::FromExternal(&tx_buffer, 1))
                .status());

  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, kTxDataSize);
  });
  ASSERT_OK(tx_completed->Wait(zx::deadline_after(zx::sec(5))));

  // 2. Queue RX Space and Receive
  auto rx_completed = std::make_shared<libsync::Completion>();
  driver_test_.RunInEnvironmentTypeContext([rx_completed](Environment& env) {
    env.fake_ifc_.set_on_complete_rx([rx_completed]() { rx_completed->Signal(); });
  });

  fnetdev::wire::RxSpaceBuffer rx_space = {
      .id = kRxBufferId,
      .region = {.vmo = kVmoId, .offset = 1024, .length = 2048},
  };
  ASSERT_OK(
      net_impl_client_.buffer(arena)
          ->QueueRxSpace(fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&rx_space, 1))
          .status());

  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, kRxDataSize);
  });
  ASSERT_OK(rx_completed->Wait(zx::deadline_after(zx::sec(5))));

  // 3. Trigger throughput and verify
  driver_test_.RunInDriverContext([kTxDataSize, kRxDataSize](UsbCdcFunction& driver) {
    driver.GetThroughputTrackerForTesting().MeasureForTesting(zx::sec(1));

    auto hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());

    auto* cdc_node = hierarchy.GetByPath({"usb-cdc-function"});
    ASSERT_NE(cdc_node, nullptr);

    const auto* online_prop = cdc_node->node().get_property<inspect::BoolPropertyValue>("online");
    ASSERT_NE(online_prop, nullptr);
    EXPECT_TRUE(online_prop->value());

    auto* bulk_in = hierarchy.GetByPath({"usb-cdc-function", "bulk_in"});
    ASSERT_NE(bulk_in, nullptr);
    auto err_in = usb_inspect::VerifyEndpointInspect(bulk_in, kTxDataSize, std::nullopt, 0,
                                                     std::nullopt, kTxDataSize);
    EXPECT_TRUE(err_in.is_ok()) << err_in.error_value();

    auto* bulk_out = hierarchy.GetByPath({"usb-cdc-function", "bulk_out"});
    ASSERT_NE(bulk_out, nullptr);
    auto err_out = usb_inspect::VerifyEndpointInspect(
        bulk_out, std::nullopt, kRxDataSize, std::nullopt, UsbCdcFunction::kRxDepth, kRxDataSize);
    EXPECT_TRUE(err_out.is_ok()) << err_out.error_value();
  });
}

// Validates CDC control requests (packet filters, clear halt) and interrupt status notifications.
TEST_F(UsbCdcTest, ControlAndNotifications) {
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // 1. Send Class-Interface request to set ethernet packet filter (acknowledged/ZX_OK)
  {
    fuchsia_hardware_usb_descriptor::UsbSetup setup{{
        .bm_request_type = USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
        .b_request = USB_CDC_SET_ETHERNET_PACKET_FILTER,
        .w_value = 0,
        .w_index = 0,
        .w_length = 0,
    }};
    auto result = function_client_->Control({setup, {}});
    ASSERT_TRUE(result.is_ok());
  }

  // 2. Send Standard-Endpoint request to clear halt feature (acknowledged/ZX_OK)
  {
    fuchsia_hardware_usb_descriptor::UsbSetup setup{{
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_CLEAR_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 0,
        .w_length = 0,
    }};
    auto result = function_client_->Control({setup, {}});
    ASSERT_TRUE(result.is_ok());
  }

  // 3. Send unsupported/invalid request (fails with ZX_ERR_NOT_SUPPORTED)
  {
    fuchsia_hardware_usb_descriptor::UsbSetup setup{{
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_ADDRESS,
        .w_value = 5,
        .w_index = 0,
        .w_length = 0,
    }};
    auto result = function_client_->Control({setup, {}});
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);
  }

  // 4. Verify Interrupt Notifications are correctly queued on kIntrEp.
  // With coalescing, SetConfigured(true) queues initial notifications (offline, speed = 0).
  // EnablePort(true) sets pending_notification_ = true while initial requests are in flight.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          pending = env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count();
        });
        return pending == 2u;
      },
      zx::sec(5)));

  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto& fake_ep = env.fake_usb_fidl_.fake_endpoint(kIntrEp);

    // Notification 1: Network Connection (Offline)
    {
      auto data_res = fake_ep.ReadPendingRequestData();
      ASSERT_OK(data_res.status_value());
      ASSERT_GE(data_res->size(), sizeof(usb_cdc_notification_t));
      usb_cdc_notification_t notif;
      std::memcpy(&notif, data_res->data(), sizeof(notif));
      EXPECT_EQ(notif.bmRequestType, USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE);
      EXPECT_EQ(notif.bNotification, USB_CDC_NC_NETWORK_CONNECTION);
      EXPECT_EQ(le16toh(notif.wValue), 0);  // Offline
      EXPECT_EQ(le16toh(notif.wLength), 0);
      fake_ep.RequestComplete(ZX_OK, sizeof(usb_cdc_notification_t));
    }

    // Notification 2: Speed Change (Offline / 0 bps)
    {
      auto data_res = fake_ep.ReadPendingRequestData();
      ASSERT_OK(data_res.status_value());
      ASSERT_GE(data_res->size(), sizeof(usb_cdc_speed_change_notification_t));
      usb_cdc_speed_change_notification_t notif;
      std::memcpy(&notif, data_res->data(), sizeof(notif));
      EXPECT_EQ(notif.notification.bmRequestType,
                USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE);
      EXPECT_EQ(notif.notification.bNotification, USB_CDC_NC_CONNECTION_SPEED_CHANGE);
      EXPECT_EQ(le16toh(notif.notification.wLength), 2 * sizeof(uint32_t));
      EXPECT_EQ(le32toh(notif.downlink_br), 0u);
      EXPECT_EQ(le32toh(notif.uplink_br), 0u);
      fake_ep.RequestComplete(ZX_OK, sizeof(usb_cdc_speed_change_notification_t));
    }
  });

  // After the first pair completes, the pending online notification pair is dispatched.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          pending = env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count();
        });
        return pending == 2u;
      },
      zx::sec(5)));

  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto& fake_ep = env.fake_usb_fidl_.fake_endpoint(kIntrEp);

    // Notification 3: Network Connection (Online)
    {
      auto data_res = fake_ep.ReadPendingRequestData();
      ASSERT_OK(data_res.status_value());
      ASSERT_GE(data_res->size(), sizeof(usb_cdc_notification_t));
      usb_cdc_notification_t notif;
      std::memcpy(&notif, data_res->data(), sizeof(notif));
      EXPECT_EQ(notif.bmRequestType, USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE);
      EXPECT_EQ(notif.bNotification, USB_CDC_NC_NETWORK_CONNECTION);
      EXPECT_EQ(le16toh(notif.wValue), 1);  // Online
      EXPECT_EQ(le16toh(notif.wLength), 0);
      fake_ep.RequestComplete(ZX_OK, sizeof(usb_cdc_notification_t));
    }

    // Notification 4: Speed Change (Online / 100 Mbps)
    {
      auto data_res = fake_ep.ReadPendingRequestData();
      ASSERT_OK(data_res.status_value());
      ASSERT_GE(data_res->size(), sizeof(usb_cdc_speed_change_notification_t));
      usb_cdc_speed_change_notification_t notif;
      std::memcpy(&notif, data_res->data(), sizeof(notif));
      EXPECT_EQ(notif.notification.bmRequestType,
                USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE);
      EXPECT_EQ(notif.notification.bNotification, USB_CDC_NC_CONNECTION_SPEED_CHANGE);
      EXPECT_EQ(le16toh(notif.notification.wLength), 2 * sizeof(uint32_t));
      EXPECT_EQ(le32toh(notif.downlink_br), 100 * 1000 * 1000u);
      EXPECT_EQ(le32toh(notif.uplink_br), 100 * 1000 * 1000u);
      fake_ep.RequestComplete(ZX_OK, sizeof(usb_cdc_speed_change_notification_t));
    }
  });

  // Let the driver process all completions and return requests to its pool.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool full = false;
        driver_test_.RunInDriverContext(
            [&](UsbCdcFunction& driver) { full = driver.IntrEpRequestsFull(); });
        return full;
      },
      zx::sec(5)));
}

// Validates that I/O completion errors on the bulk OUT endpoint are recorded in Inspect failed byte
// counters and the request is re-queued. Requires Inspect failure tracking and error re-queue logic
// in CdcRxComplete.
TEST_F(UsbCdcTest, RxCompletionFailure) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Trigger an RX completion failure (e.g. ZX_ERR_IO) on the Bulk Out endpoint.
  constexpr uint64_t kFailedRequestSize = 2048;
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_ERR_IO, kFailedRequestSize);
  });

  // Wait for the driver to handle the completion and re-queue the request.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return pending == UsbCdcFunction::kRxDepth;
      },
      zx::sec(5)));

  // Verify that the failed bytes are tracked in the Inspect metrics.
  inspect::Hierarchy hierarchy;
  driver_test_.RunInDriverContext([&](UsbCdcFunction& driver) {
    hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());
  });
  auto* bulk_out = hierarchy.GetByPath({"usb-cdc-function", "bulk_out"});
  ASSERT_NE(bulk_out, nullptr);

  auto err_out = usb_inspect::VerifyEndpointInspect(bulk_out,
                                                    /*total_bytes_tx=*/std::nullopt,
                                                    /*total_bytes_rx=*/std::nullopt,
                                                    /*tx_pending_requests=*/std::nullopt,
                                                    /*rx_pending_requests=*/std::nullopt,
                                                    /*max_bytes_per_second=*/std::nullopt,
                                                    /*rx_pending_processing=*/std::nullopt,
                                                    /*failed_bytes_tx=*/std::nullopt,
                                                    /*failed_bytes_rx=*/kFailedRequestSize);
  EXPECT_TRUE(err_out.is_ok()) << err_out.error_value();
}

// Validates that physical hardware disconnect errors (ZX_ERR_IO_NOT_PRESENT) return RX requests to
// the free pool without re-queuing. Requires CdcRxComplete to detect hardware disconnect status and
// halt re-queuing.
TEST_F(UsbCdcTest, RxCompletionHardwareDisconnected) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Trigger an RX completion with ZX_ERR_IO_NOT_PRESENT on the Bulk Out endpoint.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_ERR_IO_NOT_PRESENT, 1024);
  });

  // The driver should immediately return the request to its private pool and NOT re-queue it.
  // Therefore, the pending request count on the mock endpoint should remain (kRxDepth - 1).
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return pending == UsbCdcFunction::kRxDepth - 1;
      },
      zx::sec(5)));

  // Flush driver dispatcher to ensure CdcRxComplete has processed the completion.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {});

  // Verify no packets were delivered to the network interface.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto rx = env.fake_ifc_.PopCompleteRx();
    EXPECT_FALSE(rx.has_value());
  });
}

// Validates that cancelled RX requests (ZX_ERR_CANCELED) during teardown return to the pool without
// delivering packets. Requires CdcRxComplete to handle ZX_ERR_CANCELED cleanly.
TEST_F(UsbCdcTest, RxCompletionCanceled) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Trigger an RX completion with ZX_ERR_CANCELED on the Bulk Out endpoint.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_ERR_CANCELED, 1024);
  });

  // The driver should immediately return the request to its private pool and NOT re-queue it.
  // Therefore, the pending request count on the mock endpoint should remain (kRxDepth - 1).
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return pending == UsbCdcFunction::kRxDepth - 1;
      },
      zx::sec(5)));

  // Flush driver dispatcher to ensure CdcRxComplete has processed the completion.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {});

  // Verify no packets were delivered to the network interface.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto rx = env.fake_ifc_.PopCompleteRx();
    EXPECT_FALSE(rx.has_value());
  });
}

// Validates that RX completions arriving while the driver is transitioning to offline are safely
// discarded and returned to the pool. Requires CdcRxComplete to verify online state before
// forwarding packets.
TEST_F(UsbCdcTest, RxDiscardedWhenOffline) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // 1. Initially configured and online.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.online()); });

  // 2. Hold cancellations on the Bulk Out endpoint to simulate a race where
  // a completion arrives after the interface is disabled but before teardown finishes.
  driver_test_.RunInEnvironmentTypeContext(
      [](Environment& env) { env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(true); });
  auto cleanup_hold = fit::defer([&]() {
    driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
      env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(false);
      if (env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).has_delayed_cancel()) {
        env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).ReleaseCancelAll();
      }
    });
  });

  // 3. Put the interface into alternate setting 0 (low power / offline).
  // We run this asynchronously because SetInterface will complete when all cancelled
  // requests are returned (which is held by our mock).
  ASSERT_TRUE(function_client_.is_valid());
  auto client_end = function_client_.TakeClientEnd();

  auto set_interface_complete = std::make_shared<libsync::Completion>();
  std::shared_ptr<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      async_function_client;
  auto cleanup_client = fit::defer([&]() {
    driver_test_.RunInEnvironmentTypeContext(
        [&](Environment& env) { async_function_client.reset(); });
  });
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    async_function_client =
        std::make_shared<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>(
            std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());
    (*async_function_client)
        ->SetInterface({{
            .interface = kDataInterface,
            .alt_setting = 0,
        }})
        .Then([set_interface_complete](auto& result) {
          EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
          set_interface_complete->Signal();
        });
  });

  // Wait until the driver has set online_ to false.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool online = true;
        driver_test_.RunInDriverContext([&](UsbCdcFunction& driver) { online = driver.online(); });
        return !online;
      },
      zx::sec(5)));

  // 4. Trigger a successful RX completion (ZX_OK) on the Bulk Out endpoint
  // while the interface is offline and cancellation is in progress.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, 123);
  });

  // The driver should immediately return the request to its private pool and NOT re-queue it.
  // Therefore, the pending request count on the mock endpoint should remain (kRxDepth - 1).
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return pending == UsbCdcFunction::kRxDepth - 1;
      },
      zx::sec(5)));

  // Flush driver dispatcher to ensure CdcRxComplete has processed the completion.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {});

  // Verify that the packet was NOT delivered to the network interface.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto rx = env.fake_ifc_.PopCompleteRx();
    EXPECT_FALSE(rx.has_value());
  });

  // 5. Release the hold on cancellation so the SetInterface teardown can complete.
  cleanup_hold.call();

  ASSERT_OK(set_interface_complete->Wait(zx::deadline_after(zx::sec(5))));
  cleanup_client.call();
}

// Validates that setting alternate setting 0 disables the data plane and drains all bulk endpoint
// requests. Requires asynchronous SetInterface alt setting transition support.
TEST_F(UsbCdcTest, SetInterfaceAltSettingZero) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.online()); });

  {
    ASSERT_TRUE(function_client_.is_valid());
    fidl::Result result = function_client_->SetInterface({{
        .interface = kDataInterface,
        .alt_setting = 0,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_FALSE(driver.online()); });

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t out_pending = 0;
        size_t in_pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          out_pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
          in_pending = env.fake_usb_fidl_.fake_endpoint(kBulkInEp).pending_request_count();
        });
        return out_pending == 0 && in_pending == 0;
      },
      zx::sec(5)));

  {
    fidl::Result result = function_client_->SetInterface({{
        .interface = kDataInterface,
        .alt_setting = 1,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.online()); });

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t out_pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          out_pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return out_pending == UsbCdcFunction::kRxDepth;
      },
      zx::sec(5)));
}

TEST_F(UsbCdcTest, SetInterfaceAltSettingZeroWithBufferedRxCompletions) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // 1. Trigger an RX completion (ZX_OK) while no RX space buffers are available.
  // This will cause the driver to buffer the completion in rx_completion_queue_.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, 64);
  });

  // Wait until the driver has processed the completion and buffered it.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool has_pending = false;
        driver_test_.RunInDriverContext(
            [&](UsbCdcFunction& driver) { has_pending = driver.HasPendingRxCompletions(); });
        return has_pending;
      },
      zx::sec(5)));

  // 2. Call SetInterface(alt_setting = 0).
  // This will trigger the async teardown path, which should drain the buffered
  // completions, return their requests to the bulk out endpoint's free pool,
  // and clear the queue.
  ASSERT_TRUE(function_client_.is_valid());
  auto client_end = function_client_.TakeClientEnd();

  auto set_interface_complete = std::make_shared<libsync::Completion>();
  std::shared_ptr<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      async_function_client;
  auto cleanup_client = fit::defer([&]() {
    driver_test_.RunInEnvironmentTypeContext(
        [&](Environment& env) { async_function_client.reset(); });
  });
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    async_function_client =
        std::make_shared<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>(
            std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());
    (*async_function_client)
        ->SetInterface({{
            .interface = kDataInterface,
            .alt_setting = 0,
        }})
        .Then([set_interface_complete](auto& result) {
          EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
          set_interface_complete->Signal();
        });
  });

  // Wait until the driver has set online_ to false.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool online = true;
        driver_test_.RunInDriverContext([&](UsbCdcFunction& driver) { online = driver.online(); });
        return !online;
      },
      zx::sec(5)));

  ASSERT_OK(set_interface_complete->Wait(zx::deadline_after(zx::sec(5))));

  // Verify that the rx_completion_queue_ was cleanly cleared and the requests
  // were returned to the endpoint's free pool (so the pending count drops to 0).
  driver_test_.RunInDriverContext(
      [](UsbCdcFunction& driver) { EXPECT_FALSE(driver.HasPendingRxCompletions()); });

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t out_pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          out_pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return out_pending == 0;
      },
      zx::sec(5)));
  cleanup_client.call();
}

// Validates that calling SetInterface(alt_setting = 1) while requests are in-flight
// cleanly isolates the data plane, cancels and waits for in-flight requests without
// re-queueing completions during reconfiguration, and successfully reconfigures
// and re-arms all RX requests.
TEST_F(UsbCdcTest, SetInterfaceAltSettingOneReconfiguresSafelyWhilePending) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // 1. Initially configured and online.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.online()); });

  // 2. Hold cancellations on the Bulk Out endpoint to simulate in-flight delay.
  driver_test_.RunInEnvironmentTypeContext(
      [](Environment& env) { env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(true); });
  auto cleanup_hold = fit::defer([&]() {
    driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
      env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).set_hold_cancel(false);
      if (env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).has_delayed_cancel()) {
        env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).ReleaseCancelAll();
      }
    });
  });

  // 3. Trigger SetInterface(alt_setting = 1) asynchronously.
  ASSERT_TRUE(function_client_.is_valid());
  auto client_end = function_client_.TakeClientEnd();

  auto set_interface_complete = std::make_shared<libsync::Completion>();
  std::shared_ptr<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      async_function_client;
  auto cleanup_client = fit::defer([&]() {
    driver_test_.RunInEnvironmentTypeContext(
        [&](Environment& env) { async_function_client.reset(); });
  });
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    async_function_client =
        std::make_shared<fidl::Client<fuchsia_hardware_usb_function::UsbFunctionInterface>>(
            std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());
    (*async_function_client)
        ->SetInterface({{
            .interface = kDataInterface,
            .alt_setting = 1,
        }})
        .Then([set_interface_complete](auto& result) {
          EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
          set_interface_complete->Signal();
        });
  });

  // 4. Verify that data plane is immediately isolated (online_ is false).
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool online = true;
        driver_test_.RunInDriverContext([&](UsbCdcFunction& driver) { online = driver.online(); });
        return !online;
      },
      zx::sec(5)));

  // 5. Trigger an RX completion with error while reconfiguration barrier is waiting.
  // The driver must NOT attempt to re-queue it, but return it to the free pool.
  driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).RequestComplete(ZX_ERR_IO, 0);
  });

  // 6. Release the hold on cancellation so the barrier passes and reconfiguration completes.
  cleanup_hold.call();

  ASSERT_OK(set_interface_complete->Wait(zx::deadline_after(zx::sec(5))));

  // 7. Verify driver is online again and all kRxDepth requests are queued.
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) { EXPECT_TRUE(driver.online()); });

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t out_pending = 0;
        driver_test_.RunInEnvironmentTypeContext([&](Environment& env) {
          out_pending = env.fake_usb_fidl_.fake_endpoint(kBulkOutEp).pending_request_count();
        });
        return out_pending == UsbCdcFunction::kRxDepth;
      },
      zx::sec(5)));

  cleanup_client.call();
}

// Validates that expected peer closed errors (ZX_ERR_PEER_CLOSED) during endpoint disabling are
// cleanly handled. Requires DisableAllEndpoints to recognize expected transport disconnect errors.
TEST_F(UsbCdcTest, DisableAllEndpointsExpectedDisconnect) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Set the mock to return an expected disconnect error (ZX_ERR_PEER_CLOSED)
  // when the driver attempts to disable the endpoints during stop/teardown.
  driver_test_.RunInEnvironmentTypeContext(
      [](Environment& env) { env.fake_usb_fidl_.set_disable_endpoint_error(ZX_ERR_PEER_CLOSED); });

  // Stop the driver. This will call DisableAllEndpoints(), which should
  // hit the IsExpectedDisconnect branch and successfully log/skip it without failing.
  zx::result<> stop_result = driver_test_.StopDriver();
  ASSERT_OK(stop_result.status_value());
  driver_stopped_ = true;
}

// Validates that framework/transport-level channel closures during driver teardown do not trigger
// panics. Requires DisableAllEndpoints to handle framework transport errors safely.
TEST_F(UsbCdcTest, DisableAllEndpointsFrameworkError) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Close the UsbFunction FIDL server channel completely.
  // This will cause any subsequent calls on function_ (DisableEndpoint) to fail with
  // a framework/transport error of ZX_ERR_PEER_CLOSED.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) { env.CloseUsbFunctionServer(); });

  // Stop the driver. This will call DisableAllEndpoints(), which should
  // hit the is_framework_error() branch and get the ZX_ERR_PEER_CLOSED status,
  // identifying it as an expected disconnect and logging/skipping it.
  zx::result<> stop_result = driver_test_.StopDriver();
  ASSERT_OK(stop_result.status_value());
  driver_stopped_ = true;
}

// Validates that physical unplug disconnects (ZX_ERR_IO_NOT_PRESENT) during endpoint disabling
// succeed cleanly. Requires DisableAllEndpoints to recognize ZX_ERR_IO_NOT_PRESENT as a safe
// disconnect error.
TEST_F(UsbCdcTest, DisableAllEndpointsIoNotPresent) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // Set the mock to return ZX_ERR_IO_NOT_PRESENT simulating physical disconnect.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.set_disable_endpoint_error(ZX_ERR_IO_NOT_PRESENT);
  });

  // Stop the driver. This calls DisableAllEndpoints(), which should successfully log/skip
  // it as an expected disconnect (ZX_ERR_IO_NOT_PRESENT) and succeed.
  zx::result<> stop_result = driver_test_.StopDriver();
  ASSERT_OK(stop_result.status_value());
  driver_stopped_ = true;
}

// Validates that unbind/transport failures on endpoint channels mark the corresponding endpoint
// client dead to prevent subsequent operations. Requires dead-endpoint tracking and error callbacks
// on endpoint clients.
TEST_F(UsbCdcTest, DISABLED_EndpointFailureMarksEndpointDead) {
  StartNetworkDevice();
  ASSERT_NO_FATAL_FAILURE(SetConfiguredAndEnable());

  // 1. Close the UsbFunction server endpoint client on kBulkInEp.
  driver_test_.RunInEnvironmentTypeContext(
      [](Environment& env) { env.fake_usb_fidl_.fake_endpoint(kBulkInEp).Close(); });

  // 2. Wait deterministically for the unbind callback to run and mark the endpoint dead.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool dead = false;
        driver_test_.RunInDriverContext([&](const UsbCdcFunction& driver) {
          dead = UsbCdcTestHelper::IsTxEndpointDead(driver);
        });
        return dead;
      },
      zx::sec(5)));

  // 3. Verify the tx endpoint is marked dead in the driver context.
  driver_test_.RunInDriverContext([](const UsbCdcFunction& driver) {
    EXPECT_TRUE(UsbCdcTestHelper::IsTxEndpointDead(driver));
  });

  // 4. Deconfigure the driver to clean up and prevent dispatcher hangs in TearDown.
  {
    fidl::Result result = function_client_->SetConfigured({{
        .configured = false,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }
}

class UsbCdcNoMetadataTest : public UsbCdcTest {
 protected:
  void ConfigureMetadata(Environment& env) override {
    // Do nothing to simulate metadata not found!
  }
};

// Validates that the driver falls back to generating a random locally-administered MAC address
// when boot metadata is missing.
TEST_F(UsbCdcNoMetadataTest, FallbackToRandomMacAddress) {
  // The driver should successfully start and generate a random MAC address.
  // We can verify that the generated MAC address has the local-assignment bit (0x02) set!
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {
    std::array<uint8_t, 6> mac = driver.mac_addr();
    EXPECT_EQ(mac[0] & 0x02, 0x02);
  });
}

class UsbCdcInvalidMetadataTest : public UsbCdcTest {
 protected:
  void ConfigureMetadata(Environment& env) override {
    expect_start_success_ = false;
    // Set invalid metadata that is missing the mac_address field.
    fuchsia_boot_metadata::MacAddressMetadata metadata;
    env.mac_address_ = metadata;
  }
};

// Validates that driver startup fails cleanly when corrupted or incomplete MAC metadata is
// provided.
TEST_F(UsbCdcInvalidMetadataTest, FailsToStart) {
  // Driver fails to start in SetUp() because expect_start_success_ is false and
  // StartDriver() returns a failure, which is verified by SetUp().
}

TEST_F(UsbCdcTest, RapidInterfaceTogglesDoNotExhaustInterruptRequests) {
  StartNetworkDevice();

  // SetConfigured(true) queues the initial notification pair (network + speed).
  {
    fidl::Result result = function_client_->SetConfigured({{
        .configured = true,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Verify initial in-flight count is 2 and 2 requests are pending in fake endpoint.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInEnvironmentTypeContext([&ready](Environment& env) {
          ready = (env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count() == 2);
        });
        return ready;
      },
      zx::sec(5)));

  // Rapidly toggle interface 10 times without completing any interrupt requests on kIntrEp.
  // This simulates Pontis / WebUSB host behavior where endpoint 0x81 is never polled.
  for (int i = 0; i < 10; ++i) {
    {
      fidl::Result result = function_client_->SetInterface({{
          .interface = kDataInterface,
          .alt_setting = 1,
      }});
      ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
    }
    {
      fidl::Result result = function_client_->SetInterface({{
          .interface = kDataInterface,
          .alt_setting = 0,
      }});
      ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
    }
  }

  // Verify that intr_ep_ in_flight never exceeded 2 and the online Inspect property reflects the
  // final interface state (false).
  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {
    auto hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());
    auto* cdc_node = hierarchy.GetByPath({"usb-cdc-function"});
    ASSERT_TRUE(cdc_node != nullptr);

    const auto* online_prop = cdc_node->node().get_property<inspect::BoolPropertyValue>("online");
    ASSERT_TRUE(online_prop != nullptr);
    EXPECT_FALSE(online_prop->value());
  });

  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    EXPECT_EQ(env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count(), 2u);
  });

  // One more toggle to alt_setting 1 to verify online property transitions to true.
  {
    fidl::Result result = function_client_->SetInterface({{
        .interface = kDataInterface,
        .alt_setting = 1,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  driver_test_.RunInDriverContext([](UsbCdcFunction& driver) {
    auto hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());
    auto* cdc_node = hierarchy.GetByPath({"usb-cdc-function"});
    ASSERT_TRUE(cdc_node != nullptr);

    const auto* online_prop = cdc_node->node().get_property<inspect::BoolPropertyValue>("online");
    ASSERT_TRUE(online_prop != nullptr);
    EXPECT_TRUE(online_prop->value());
  });
}

TEST_F(UsbCdcTest, PendingNotificationDispatchedOnCompletion) {
  StartNetworkDevice();

  // SetConfigured(true) queues initial notifications with online = false.
  {
    fidl::Result result = function_client_->SetConfigured({{
        .configured = true,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Wait for initial notification pair to arrive at fake endpoint.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInEnvironmentTypeContext([&ready](Environment& env) {
          ready = (env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count() == 2);
        });
        return ready;
      },
      zx::sec(5)));

  // Verify initial in-flight request has network_notification wValue = 0 (offline).
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    ASSERT_EQ(env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count(), 2u);
    auto data = env.fake_usb_fidl_.fake_endpoint(kIntrEp).ReadPendingRequestData();
    ASSERT_TRUE(data.is_ok());
    ASSERT_GE(data->size(), sizeof(usb_cdc_notification_t));
    const auto* notif = reinterpret_cast<const usb_cdc_notification_t*>(data->data());
    EXPECT_EQ(notif->bNotification, USB_CDC_NC_NETWORK_CONNECTION);
    EXPECT_EQ(notif->wValue, 0u);
  });

  // Toggle interface to alt_setting 1 while initial notification is in flight.
  // This should set pending_notification_ = true without queueing additional requests.
  {
    fidl::Result result = function_client_->SetInterface({{
        .interface = kDataInterface,
        .alt_setting = 1,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Pending request count in fake endpoint should still be 2 (no extra requests queued).
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    EXPECT_EQ(env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count(), 2u);
  });

  // Complete the first in-flight request on fake_endpoint(kIntrEp).
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kIntrEp).RequestComplete(ZX_OK,
                                                              sizeof(usb_cdc_notification_t));
  });

  // Wait for the first completion to be processed by the driver.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInDriverContext([&ready](UsbCdcFunction& driver) {
          ready = (driver.GetInFlightInterruptCountForTesting() == 1u);
        });
        return ready;
      },
      zx::sec(5)));

  // With only 1 request completed, in_flight should be 1 and pending notification should NOT
  // have been dispatched yet.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    EXPECT_EQ(env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count(), 1u);
  });

  // Now complete the second in-flight request.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    env.fake_usb_fidl_.fake_endpoint(kIntrEp).RequestComplete(
        ZX_OK, sizeof(usb_cdc_speed_change_notification_t));
  });

  // Wait deterministically for CdcIntrComplete to return requests and re-dispatch
  // the pending notification pair (which will queue 2 new requests to kIntrEp).
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInEnvironmentTypeContext([&ready](Environment& env) {
          ready = (env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count() == 2);
        });
        return ready;
      },
      zx::sec(5)));

  // Wait for driver to have both new requests marked in-flight.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInDriverContext([&ready](UsbCdcFunction& driver) {
          ready = (driver.GetInFlightInterruptCountForTesting() == 2u);
        });
        return ready;
      },
      zx::sec(5)));

  // Verify that the newly dispatched notification has updated status (wValue = 1, online).
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    auto data = env.fake_usb_fidl_.fake_endpoint(kIntrEp).ReadPendingRequestData();
    ASSERT_TRUE(data.is_ok());
    ASSERT_GE(data->size(), sizeof(usb_cdc_notification_t));
    const auto* notif = reinterpret_cast<const usb_cdc_notification_t*>(data->data());
    EXPECT_EQ(notif->bNotification, USB_CDC_NC_NETWORK_CONNECTION);
    EXPECT_EQ(notif->wValue, 1u);
  });
}

TEST_F(UsbCdcTest, SetConfiguredFalseClearsPendingNotification) {
  StartNetworkDevice();

  // SetConfigured(true) queues initial notification pair.
  {
    fidl::Result result = function_client_->SetConfigured({{
        .configured = true,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Toggle interface to alt_setting 1 to set pending_notification_ = true.
  {
    fidl::Result result = function_client_->SetInterface({{
        .interface = kDataInterface,
        .alt_setting = 1,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Deconfigure device: SetConfigured(false).
  // This should reset pending_notification_ = false and disable endpoints.
  {
    fidl::Result result = function_client_->SetConfigured({{
        .configured = false,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  // Wait for cancelled requests to complete and return to intr_ep_.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        bool ready = false;
        driver_test_.RunInDriverContext([&ready](UsbCdcFunction& driver) {
          ready = (driver.GetInFlightInterruptCountForTesting() == 0u);
        });
        return ready;
      },
      zx::sec(5)));

  // Verify that all requests have been returned to intr_ep_ (0 in flight)
  // and no new requests were dispatched after deconfiguration.
  driver_test_.RunInEnvironmentTypeContext([](Environment& env) {
    EXPECT_EQ(env.fake_usb_fidl_.fake_endpoint(kIntrEp).pending_request_count(), 0u);
  });
}

}  // namespace
}  // namespace usb_cdc_function
