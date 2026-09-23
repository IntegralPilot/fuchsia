// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adb-function.h"

#include <fidl/fuchsia.hardware.adb/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sync/completion.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <vector>

#include <gtest/gtest.h>
#include <usb-inspect/usb-inspect-test-helper.h>
#include <usb/usb-request.h>

#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"

namespace usb_adb_function {

class UsbAdbTestHelper {
 public:
  static State state(const UsbAdbDevice& device) { return device.state_; }
  static void set_state(UsbAdbDevice& device, State state) { device.state_ = state; }
  static bool has_usb_function_binding(const UsbAdbDevice& device) {
    return device.usb_function_binding_.has_value();
  }
  static bool CancelAllCompleted(const UsbAdbDevice& device) { return device.CancelAllCompleted(); }
  static bool AllRequestsReturned(UsbAdbDevice& device) { return device.AllRequestsReturned(); }
  static bool InternalPoolsFull(UsbAdbDevice& device) {
    return device.bulk_out_ep_.RequestsFull() && device.bulk_in_ep_.RequestsFull();
  }
  static size_t BulkInEpPendingRequests(UsbAdbDevice& device) {
    return device.bulk_in_ep_.GetInFlightCount();
  }
  static size_t TxPendingReqsCount(const UsbAdbDevice& device) {
    return device.tx_pending_reqs_.size();
  }
  static size_t RxRequestsCount(const UsbAdbDevice& device) { return device.rx_requests_.size(); }
  static size_t PendingRepliesCount(const UsbAdbDevice& device) {
    return device.pending_replies_.size();
  }
  static inspect::Inspector GetInspector(UsbAdbDevice& device) {
    if (device.component_inspector_.has_value()) {
      return device.component_inspector_->inspector();
    }
    return {};
  }

  static usb_inspect::ThroughputTracker* GetThroughputTracker(UsbAdbDevice& device) {
    if (device.throughput_tracker_.has_value()) {
      return &*device.throughput_tracker_;
    }
    return nullptr;
  }
};

static constexpr uint32_t kBulkOutEp = 1;
static constexpr uint32_t kBulkInEp = 2;

class DelayedCancelEndpoint : public fake_usb_endpoint::FakeEndpoint {
 public:
  void CancelAll(CancelAllCompleter::Sync& completer) override {
    {
      std::lock_guard<std::mutex> _(lock_);
      if (cancel_status_ != ZX_OK) {
        completer.Reply(fit::error(cancel_status_));
        return;
      }
      if (hold_cancel_) {
        delayed_cancels_.push_back(completer.ToAsync());
        return;
      }
    }
    DoCancelAll(completer.ToAsync());
  }

  void ReleaseCancelAll() {
    std::vector<CancelAllCompleter::Async> completers;
    {
      std::lock_guard<std::mutex> _(lock_);
      completers = std::move(delayed_cancels_);
      delayed_cancels_.clear();
    }
    for (auto& completer : completers) {
      DoCancelAll(std::move(completer));
    }
  }

  void set_hold_cancel(bool hold) {
    std::lock_guard<std::mutex> _(lock_);
    hold_cancel_ = hold;
  }

  void set_cancel_status(zx_status_t status) {
    std::lock_guard<std::mutex> _(lock_);
    cancel_status_ = status;
  }

 private:
  void DoCancelAll(CancelAllCompleter::Async completer) {
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    {
      std::lock_guard<std::mutex> _(lock_);
      local_binding = binding_ref_;
      std::vector<fuchsia_hardware_usb_request::Request> pending_requests = std::move(requests_);
      requests_.clear();
      completions.reserve(pending_requests.size());
      for (auto& request : pending_requests) {
        completions.emplace_back(std::move(fuchsia_hardware_usb_endpoint::Completion()
                                               .request(std::move(request))
                                               .status(ZX_ERR_CANCELED)
                                               .transfer_size(0)));
      }
    }
    if (!completions.empty() && local_binding.has_value()) {
      EXPECT_TRUE(fidl::SendEvent(*local_binding)->OnCompletion(std::move(completions)).is_ok());
    }
    completer.Reply(fit::ok());
  }

  zx_status_t cancel_status_ __TA_GUARDED(lock_) = ZX_OK;
  bool hold_cancel_ __TA_GUARDED(lock_) = false;
  std::vector<CancelAllCompleter::Async> delayed_cancels_ __TA_GUARDED(lock_);
};

class AdbFakeUsb
    : public fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction,
                                                    DelayedCancelEndpoint> {
 public:
  using Base = fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction,
                                                      DelayedCancelEndpoint>;
  AdbFakeUsb(async_dispatcher_t* dispatcher) : Base(dispatcher), dispatcher_(dispatcher) {}

  void AllocResources(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::AllocResources>::Sync& completer) override {
    zx_status_t alloc_status = ZX_OK;
    bool omit_eps = false;
    {
      std::lock_guard<std::mutex> _(lock_);
      alloc_status = alloc_resources_status_;
      omit_eps = omit_endpoints_;
    }
    if (alloc_status != ZX_OK) {
      completer.Reply(fit::error(alloc_status));
      return;
    }

    fuchsia_hardware_usb_function::UsbFunctionAllocResourcesResponse response;
    EXPECT_EQ(request.endpoints().size(), 2u);
    EXPECT_EQ(request.interface_count(), 1u);
    EXPECT_EQ(request.strings().size(), 0u);
    response.interface_nums() = {0};
    if (omit_eps) {
      response.endpoint_addrs() = {kBulkOutEp};
    } else {
      response.endpoint_addrs() = {kBulkOutEp, kBulkInEp};
    }
    response.string_indices() = {};

    size_t count = std::min(request.endpoints().size(), response.endpoint_addrs().size());
    for (size_t i = 0; i < count; i++) {
      uint8_t addr = response.endpoint_addrs()[i];
      fake_endpoint(addr).Connect(dispatcher_, std::move(request.endpoints()[i].endpoint()));
    }

    completer.Reply(fit::ok(std::move(response)));
  }

  void Configure(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure>& request,
      fidl::internal::NaturalCompleter<fuchsia_hardware_usb_function::UsbFunction::Configure>::Sync&
          completer) override {
    fit::callback<void()> cb;
    {
      std::lock_guard<std::mutex> _(lock_);
      if (configure_status_ != ZX_OK) {
        completer.Reply(fit::error(configure_status_));
        return;
      }
      if (iface_client_.is_valid()) {
        completer.Reply(fit::error(ZX_ERR_ALREADY_BOUND));
        return;
      }
      iface_client_ = std::move(request.iface());
      configured_ = true;
      deconfigured_ = false;
      cb = std::move(on_configured_);
    }
    completer.Reply(fit::ok());
    if (cb) {
      cb();
    }
  }

  void Deconfigure(
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Sync& completer) override {
    fit::callback<void()> cb;
    {
      std::lock_guard<std::mutex> _(lock_);
      deconfigured_ = true;
      configured_ = false;
      if (hold_deconfigure_) {
        delayed_deconfigure_completer_ = completer.ToAsync();
        return;
      }
      cb = std::move(on_deconfigured_);
    }
    completer.Reply(fit::ok());
    if (cb) {
      cb();
    }
  }

  bool is_configured() const {
    std::lock_guard<std::mutex> _(lock_);
    return configured_;
  }
  void set_hold_deconfigure(bool hold) {
    std::lock_guard<std::mutex> _(lock_);
    hold_deconfigure_ = hold;
  }

  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> TakeIfaceClient() {
    std::lock_guard<std::mutex> _(lock_);
    return std::move(iface_client_);
  }

  void set_on_configured(fit::callback<void()> on_configured) {
    bool call_now = false;
    {
      std::lock_guard<std::mutex> _(lock_);
      if (configured_) {
        call_now = true;
      } else {
        on_configured_ = std::move(on_configured);
      }
    }
    if (call_now && on_configured) {
      on_configured();
    }
  }

  void set_on_deconfigured(fit::callback<void()> on_deconfigured) {
    bool call_now = false;
    {
      std::lock_guard<std::mutex> _(lock_);
      if (deconfigured_) {
        call_now = true;
      } else {
        on_deconfigured_ = std::move(on_deconfigured);
      }
    }
    if (call_now && on_deconfigured) {
      on_deconfigured();
    }
  }

  void DisableEndpoint(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>::Sync& completer) override {
    {
      std::lock_guard<std::mutex> _(lock_);
      disabled_endpoints_.push_back(request.endpoint_address());
    }
    Base::DisableEndpoint(request, completer);
  }

  std::vector<uint8_t> disabled_endpoints() const {
    std::lock_guard<std::mutex> _(lock_);
    return disabled_endpoints_;
  }
  void clear_disabled_endpoints() {
    std::lock_guard<std::mutex> _(lock_);
    disabled_endpoints_.clear();
  }

  void set_alloc_resources_status(zx_status_t status) {
    std::lock_guard<std::mutex> _(lock_);
    alloc_resources_status_ = status;
  }
  void set_configure_status(zx_status_t status) {
    std::lock_guard<std::mutex> _(lock_);
    configure_status_ = status;
  }
  void set_omit_endpoints(bool omit) {
    std::lock_guard<std::mutex> _(lock_);
    omit_endpoints_ = omit;
  }

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> iface_client_
      __TA_GUARDED(lock_);
  fit::callback<void()> on_configured_ __TA_GUARDED(lock_);
  fit::callback<void()> on_deconfigured_ __TA_GUARDED(lock_);
  zx_status_t alloc_resources_status_ __TA_GUARDED(lock_) = ZX_OK;
  zx_status_t configure_status_ __TA_GUARDED(lock_) = ZX_OK;
  bool omit_endpoints_ __TA_GUARDED(lock_) = false;
  bool hold_deconfigure_ __TA_GUARDED(lock_) = false;
  std::optional<fidl::internal::NaturalCompleter<
      fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Async>
      delayed_deconfigure_completer_ __TA_GUARDED(lock_);
  bool deconfigured_ __TA_GUARDED(lock_) = false;
  bool configured_ __TA_GUARDED(lock_) = false;
  std::vector<uint8_t> disabled_endpoints_ __TA_GUARDED(lock_);
  mutable std::mutex lock_;
};

class UsbAdbEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    fake_dev_.emplace(dispatcher);

    fake_dev_->set_alloc_resources_status(alloc_resources_status_);
    fake_dev_->set_configure_status(configure_status_);
    fake_dev_->set_omit_endpoints(omit_endpoints_);

    fuchsia_hardware_usb_function::UsbFunctionService::InstanceHandler handler({
        .device = usb_function_bindings_.CreateHandler(&*fake_dev_, dispatcher,
                                                       fidl::kIgnoreBindingClosure),
    });
    EXPECT_TRUE(
        to_driver_vfs
            .AddService<fuchsia_hardware_usb_function::UsbFunctionService>(std::move(handler))
            .is_ok());

    return zx::ok();
  }

  void set_alloc_resources_status(zx_status_t status) { alloc_resources_status_ = status; }
  void set_configure_status(zx_status_t status) { configure_status_ = status; }
  void set_omit_endpoints(bool omit) { omit_endpoints_ = omit; }

  void CancelAllUsbRequests() {
    if (!fake_dev_.has_value()) {
      return;
    }
    while (fake_dev_->fake_endpoint(kBulkOutEp).pending_request_count() > 0) {
      fake_dev_->fake_endpoint(kBulkOutEp).RequestComplete(ZX_ERR_CANCELED, 0);
    }
    while (fake_dev_->fake_endpoint(kBulkInEp).pending_request_count() > 0) {
      fake_dev_->fake_endpoint(kBulkInEp).RequestComplete(ZX_ERR_CANCELED, 0);
    }
  }

  std::optional<AdbFakeUsb> fake_dev_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_function::UsbFunction> usb_function_bindings_;
  zx_status_t alloc_resources_status_ = ZX_OK;
  zx_status_t configure_status_ = ZX_OK;
  bool omit_endpoints_ = false;
};

class UsbAdbTestConfig final {
 public:
  using DriverType = UsbAdbDevice;
  using EnvironmentType = UsbAdbEnvironment;
};

class EventHandler : public fidl::WireSyncEventHandler<fadb::UsbAdbImpl> {
 public:
  ~EventHandler() { EXPECT_TRUE(expected_statuses_.empty()); }

  void OnStatusChanged(fidl::WireEvent<fadb::UsbAdbImpl::OnStatusChanged>* event) override {
    ASSERT_FALSE(expected_statuses_.empty());
    EXPECT_EQ(event->status, expected_statuses_.front());
    expected_statuses_.pop();
  }

  std::queue<fadb::StatusFlags> expected_statuses_;
};

class UsbAdbTest : public testing::Test {
 public:
  void ReadInspect(const inspect::Inspector& inspector) {
    hierarchy_ = usb_inspect::ReadHierarchyFromInspector(inspector);
  }
  const inspect::Hierarchy& hierarchy() const { return *hierarchy_; }

  std::optional<inspect::Hierarchy> hierarchy_;

  fidl::WireSyncClient<fadb::UsbAdbImpl> NormalStartAdb() {
    auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
    EXPECT_TRUE(client_->StartAdb(std::move(server_end)).ok());
    WaitConfigured();
    EnableUsb();

    return fidl::WireSyncClient<fadb::UsbAdbImpl>(std::move(client_end));
  }

  void SetUp() override {
    ASSERT_EQ(driver_test_.StartDriver().status_value(), ZX_OK);

    driver_test_.runtime().RunUntil([&]() {
      bool ready = false;
      driver_test_.RunInEnvironmentTypeContext(
          [&](UsbAdbEnvironment& env) { ready = env.fake_dev_->is_configured(); });
      return ready;
    });

    auto device = driver_test_.Connect<fadb::Service::Adb>();
    EXPECT_EQ(device.status_value(), ZX_OK);
    client_.Bind(std::move(device.value()));

    driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
      EXPECT_EQ(dev.bulk_out_addr(), kBulkOutEp);
      EXPECT_EQ(dev.bulk_in_addr(), kBulkInEp);
    });
  }

  void TearDown() override {
    client_ = {};
    iface_client_ = {};

    SafeStopDriver();

    driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
      if (env.fake_dev_.has_value()) {
        env.fake_dev_->set_on_configured(nullptr);
        env.fake_dev_->set_on_deconfigured(nullptr);
        env.fake_dev_.reset();
      }
      env.usb_function_bindings_.RemoveAll();
    });

    driver_test_.runtime().RunUntilIdle();
  }

  void EnsureIfaceBound() { WaitConfigured(); }

  void EnableUsb() {
    EnsureIfaceBound();
    ASSERT_TRUE(iface_client_.is_valid());
    fidl::Result result = iface_client_->SetConfigured({{
        .configured = true,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kFull,
    }});
    EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();

    ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
        [&]() {
          size_t count = 0;
          driver_test_.RunInEnvironmentTypeContext([&count](UsbAdbEnvironment& env) {
            count = env.fake_dev_->fake_endpoint(kBulkOutEp).pending_request_count();
          });
          bool is_online = false;
          driver_test_.RunInDriverContext([&is_online](UsbAdbDevice& dev) {
            is_online = (UsbAdbTestHelper::state(dev) == State::kOnline);
          });
          return (count == kBulkRxCount) && is_online;
        },
        zx::sec(5)));
  }

  void WaitConfigured() {
    if (iface_client_.is_valid()) {
      return;
    }
    fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> iface_client;
    for (;;) {
      libsync::Completion configured;
      driver_test_.RunInEnvironmentTypeContext(
          [&iface_client, &configured](UsbAdbEnvironment& env) {
            if (!env.fake_dev_.has_value()) {
              return;
            }
            iface_client = env.fake_dev_->TakeIfaceClient();
            if (iface_client.is_valid()) {
              env.fake_dev_->set_on_configured(nullptr);
              return;
            }
            env.fake_dev_->set_on_configured([&configured]() { configured.Signal(); });
          });
      if (iface_client.is_valid()) {
        iface_client_.Bind(std::move(iface_client));
        return;
      }
      configured.Wait();
      driver_test_.RunInEnvironmentTypeContext([&](UsbAdbEnvironment& env) {
        if (env.fake_dev_.has_value()) {
          env.fake_dev_->set_on_configured(nullptr);
        }
      });
    }
  }

  void SafeStopDriver() {
    if (driver_stopped_) {
      return;
    }
    driver_test_.runtime().RunUntilIdle();
    CancelAllUsbRequestsOnDeconfigure();
    EXPECT_TRUE(driver_test_.StopDriver().is_ok());
    driver_stopped_ = true;
  }

  void WaitForState(State target_state) {
    ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
        [&]() {
          bool match = false;
          driver_test_.RunInDriverContext(
              [&](UsbAdbDevice& dev) { match = (UsbAdbTestHelper::state(dev) == target_state); });
          return match;
        },
        zx::sec(5)));
  }

  void ExpectHandleOneEventSafe(fidl::WireSyncClient<fadb::UsbAdbImpl>& client,
                                EventHandler& handler) {
    while (true) {
      auto result = client.HandleOneEvent(handler);
      if (result.status() == ZX_ERR_PEER_CLOSED) {
        break;
      }
      ASSERT_EQ(result.status(), ZX_OK);
      if (handler.expected_statuses_.empty()) {
        break;
      }
    }
  }

  void SendTestData(fidl::WireSyncClient<fadb::UsbAdbImpl>& usb_impl, size_t size) {
    std::vector<uint8_t> test_data(size);

    driver_test_.RunInEnvironmentTypeContext([&test_data](UsbAdbEnvironment& env) {
      for (uint32_t i = 0; i < test_data.size() / kVmoDataSize; i++) {
        env.fake_dev_->fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, kVmoDataSize);
      }
      if (test_data.size() % kVmoDataSize) {
        env.fake_dev_->fake_endpoint(kBulkInEp).RequestComplete(ZX_OK,
                                                                test_data.size() % kVmoDataSize);
      }
    });

    auto result = usb_impl->QueueTx(
        fidl::VectorView<uint8_t>::FromExternal(test_data.data(), test_data.size()));
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());

    driver_test_.runtime().RunUntil([&] {
      bool empty = false;
      driver_test_.RunInEnvironmentTypeContext([&](UsbAdbEnvironment& env) {
        empty = (env.fake_dev_->fake_endpoint(kBulkInEp).pending_request_count() == 0u);
      });
      if (!empty) {
        return false;
      }
      driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
        empty = (UsbAdbTestHelper::BulkInEpPendingRequests(dev) == 0u);
      });
      return empty;
    });
  }

  void ExpectReceiveData(size_t size) {
    driver_test_.RunInEnvironmentTypeContext([size](UsbAdbEnvironment& env) {
      env.fake_dev_->fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, size);
    });
  }

  void CancelAllUsbRequestsOnDeconfigure() {
    driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
      if (env.fake_dev_.has_value()) {
        env.fake_dev_->set_on_deconfigured([&]() { env.CancelAllUsbRequests(); });
      }
    });
  }

  fdf_testing::BackgroundDriverTest<UsbAdbTestConfig> driver_test_;
  fidl::WireSyncClient<fadb::Device> client_;
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunctionInterface> iface_client_;
  bool driver_stopped_ = false;
};

TEST_F(UsbAdbTest, StopBeforeUsbStartsUp) {
  driver_test_.runtime().RunUntil([&]() {
    bool awaiting = false;
    driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
      awaiting = (UsbAdbTestHelper::state(dev) == State::kAwaitingUsbConnection);
    });
    return awaiting;
  });
  driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
    EXPECT_EQ(UsbAdbTestHelper::state(dev), State::kAwaitingUsbConnection);
  });
  SafeStopDriver();
}

TEST_F(UsbAdbTest, StartStop) {
  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  EXPECT_TRUE(client_->StartAdb(std::move(server_end)).ok());
  auto usb_impl = fidl::WireSyncClient<fadb::UsbAdbImpl>(std::move(client_end));

  EventHandler handler;

  EnableUsb();

  handler.expected_statuses_.push(fadb::StatusFlags::kOnline);
  ExpectHandleOneEventSafe(usb_impl, handler);

  libsync::Completion stop_requested;
  driver_test_.RunInEnvironmentTypeContext([&stop_requested](UsbAdbEnvironment& env) {
    env.fake_dev_->set_on_deconfigured([&stop_requested]() { stop_requested.Signal(); });
  });

  std::atomic<bool> stop_finished = false;
  auto dev_client_end = client_.TakeClientEnd();
  fidl::WireClient async_client(std::move(dev_client_end),
                                fdf::Dispatcher::GetCurrent()->async_dispatcher());

  async_client->StopAdb().ThenExactlyOnce([&stop_finished](auto& result) {
    EXPECT_TRUE(result.ok() || result.status() == ZX_ERR_PEER_CLOSED);
    stop_finished = true;
  });

  driver_test_.RunInEnvironmentTypeContext(
      [&](UsbAdbEnvironment& env) { env.CancelAllUsbRequests(); });
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil([&]() { return stop_finished.load(); },
                                                           zx::sec(5)));

  driver_test_.RunInEnvironmentTypeContext(
      [&](UsbAdbEnvironment& env) { env.fake_dev_->set_on_deconfigured(nullptr); });

  usb_impl = {};
  async_client = {};

  SafeStopDriver();
}

TEST_F(UsbAdbTest, StopDriverWhileConnected) {
  auto usb_impl = NormalStartAdb();

  EventHandler handler;
  handler.expected_statuses_.emplace(fadb::StatusFlags::kOnline);

  ExpectHandleOneEventSafe(usb_impl, handler);

  CancelAllUsbRequestsOnDeconfigure();
  usb_impl = {};

  SafeStopDriver();
}

TEST_F(UsbAdbTest, UsbStackRequestsStop) {
  auto usb_impl = NormalStartAdb();

  EventHandler handler;
  handler.expected_statuses_.emplace(fadb::StatusFlags::kOnline);

  ExpectHandleOneEventSafe(usb_impl, handler);

  auto iface_client_end = iface_client_.TakeClientEnd();
  fidl::WireClient async_iface(std::move(iface_client_end),
                               fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::atomic<bool> set_configured_finished = false;
  async_iface->SetConfigured(false, fuchsia_hardware_usb_descriptor::wire::UsbSpeed::kFull)
      .ThenExactlyOnce([&](auto& result) {
        EXPECT_TRUE(result.ok() || result.status() == ZX_ERR_PEER_CLOSED);
        set_configured_finished = true;
      });

  driver_test_.RunInEnvironmentTypeContext(
      [](UsbAdbEnvironment& env) { env.CancelAllUsbRequests(); });

  handler.expected_statuses_.emplace(0);

  ExpectHandleOneEventSafe(usb_impl, handler);

  usb_impl = {};

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&] { return set_configured_finished.load(); }, zx::sec(5)));

  SafeStopDriver();
}

TEST_F(UsbAdbTest, SendAdbMessage) {
  auto usb_impl = NormalStartAdb();

  ASSERT_NO_FATAL_FAILURE(SendTestData(usb_impl, kVmoDataSize - 2));
  ASSERT_NO_FATAL_FAILURE(SendTestData(usb_impl, kVmoDataSize));
  ASSERT_NO_FATAL_FAILURE(SendTestData(usb_impl, kVmoDataSize + 2));
  ASSERT_NO_FATAL_FAILURE(SendTestData(usb_impl, kVmoDataSize * (kBulkTxCount - 2) + 2));
  ASSERT_NO_FATAL_FAILURE(SendTestData(usb_impl, kVmoDataSize * (kBulkTxCount - 1) + 2));

  SafeStopDriver();
}

TEST_F(UsbAdbTest, RecvAdbMessage) {
  constexpr uint32_t kReceiveSize = kVmoDataSize - 2;
  auto usb_impl = NormalStartAdb();

  auto dev_client_end = usb_impl.TakeClientEnd();
  fidl::WireClient async_usb(std::move(dev_client_end),
                             fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::atomic<bool> completed = false;
  size_t actual_size = 0;
  zx_status_t response_status = ZX_ERR_INTERNAL;
  async_usb->Receive().ThenExactlyOnce([&](auto& response) {
    completed = true;
    response_status = response.status();
    if (response.ok()) {
      actual_size = response.value().value()->data.size();
    }
  });

  driver_test_.runtime().RunUntilIdle();

  EXPECT_NO_FATAL_FAILURE(ExpectReceiveData(kReceiveSize));
  ASSERT_TRUE(
      driver_test_.runtime().RunWithTimeoutOrUntil([&] { return completed.load(); }, zx::sec(5)));
  EXPECT_EQ(response_status, ZX_OK);
  EXPECT_EQ(actual_size, kReceiveSize);

  SafeStopDriver();
}

TEST_F(UsbAdbTest, VerifyShutdownCompletesWhenHardwareUnresponsive) {
  auto usb_impl = NormalStartAdb();
  EventHandler handler;
  handler.expected_statuses_.emplace(fadb::StatusFlags::kOnline);
  ExpectHandleOneEventSafe(usb_impl, handler);

  auto iface_client_end = iface_client_.TakeClientEnd();
  ASSERT_TRUE(iface_client_end.is_valid());
  fidl::WireClient async_iface(std::move(iface_client_end),
                               fdf::Dispatcher::GetCurrent()->async_dispatcher());

  async_iface->SetConfigured(false, fuchsia_hardware_usb_descriptor::wire::UsbSpeed::kHigh)
      .ThenExactlyOnce([&](auto& result) {
        EXPECT_TRUE(result.ok() || result.status() == ZX_ERR_PEER_CLOSED ||
                    result.status() == ZX_ERR_CANCELED);
      });

  driver_test_.runtime().RunUntilIdle();

  CancelAllUsbRequestsOnDeconfigure();

  // Simulate hardware power-off or inactive controller endpoints where CancelAll
  // returns ZX_ERR_IO_NOT_PRESENT. The driver should handle the error and complete shutdown.
  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_cancel_status(ZX_ERR_IO_NOT_PRESENT);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_cancel_status(ZX_ERR_IO_NOT_PRESENT);
  });

  EXPECT_TRUE(driver_test_.StopDriver().is_ok());
  driver_stopped_ = true;
}

TEST_F(UsbAdbTest, VerifyInspect) {
  auto usb_impl = NormalStartAdb();

  // Queue some tx packets
  ASSERT_NO_FATAL_FAILURE(SendTestData(usb_impl, 100));

  auto dev_client_end = usb_impl.TakeClientEnd();
  fidl::WireClient async_usb(std::move(dev_client_end),
                             fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::atomic<bool> completed = false;
  size_t actual_size = 0;
  zx_status_t response_status = ZX_ERR_INTERNAL;
  async_usb->Receive().ThenExactlyOnce([&](auto& response) {
    completed = true;
    response_status = response.status();
    if (response.ok()) {
      actual_size = response.value().value()->data.size();
    }
  });

  driver_test_.runtime().RunUntilIdle();

  EXPECT_NO_FATAL_FAILURE(ExpectReceiveData(200));
  ASSERT_TRUE(
      driver_test_.runtime().RunWithTimeoutOrUntil([&] { return completed.load(); }, zx::sec(5)));
  EXPECT_EQ(response_status, ZX_OK);
  EXPECT_EQ(actual_size, 200u);

  // Fetch inspector from driver
  inspect::Inspector inspector;
  driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
    if (auto* tracker = UsbAdbTestHelper::GetThroughputTracker(dev)) {
      tracker->MeasureForTesting(zx::sec(1));
    }
    inspector = UsbAdbTestHelper::GetInspector(dev);
  });

  ASSERT_NO_FATAL_FAILURE(ReadInspect(inspector));

  // Verify adb function inspect node exists
  auto* root_node = this->hierarchy().GetByPath({"usb-adb-function"});
  ASSERT_NE(nullptr, root_node);

  // Verify state string
  const auto* state_prop = root_node->node().get_property<inspect::StringPropertyValue>("state");
  ASSERT_NE(nullptr, state_prop);
  EXPECT_EQ("kOnline", state_prop->value());

  // Verify online boolean property
  const auto* online_prop = root_node->node().get_property<inspect::BoolPropertyValue>("online");
  ASSERT_NE(nullptr, online_prop);
  EXPECT_TRUE(online_prop->value());

  // Verify bulk_in (TX) stats using the shared helper
  auto* bulk_in = this->hierarchy().GetByPath({"usb-adb-function", "bulk_in"});
  ASSERT_NE(nullptr, bulk_in);
  auto err_in =
      usb_inspect::VerifyEndpointInspect(bulk_in, 100, std::nullopt, 0, std::nullopt, 100, 0);
  EXPECT_TRUE(err_in.is_ok()) << err_in.error_value();

  // Verify bulk_out (RX) stats using the shared helper
  auto* bulk_out = this->hierarchy().GetByPath({"usb-adb-function", "bulk_out"});
  ASSERT_NE(nullptr, bulk_out);
  auto err_out =
      usb_inspect::VerifyEndpointInspect(bulk_out, std::nullopt, 200, std::nullopt, 0, 200, 0);
  EXPECT_TRUE(err_out.is_ok()) << err_out.error_value();

  // Verify events are logged
  auto* event_history =
      this->hierarchy().GetByPath({"usb-adb-function", "bulk_in", "event_history"});
  ASSERT_NE(nullptr, event_history);
  EXPECT_GT(event_history->children().size(), 0u);

  async_usb = {};
}

TEST_F(UsbAdbTest, QueueTxZeroLengthRejected) {
  // TODO(b/395982869): Move this test to a separate error-expecting test file/suite after all
  // this is done so we don't have to degrade the max_severity checks for other tests.
  auto usb_impl_sync = NormalStartAdb();
  auto dev_client_end = usb_impl_sync.TakeClientEnd();
  fidl::WireClient async_usb(std::move(dev_client_end),
                             fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Queue a zero-length vector, which should be rejected.
  std::atomic<bool> completed = false;
  async_usb->QueueTx(fidl::VectorView<uint8_t>()).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_error());
    EXPECT_EQ(result->error_value(), ZX_ERR_INVALID_ARGS);
    completed = true;
  });

  ASSERT_TRUE(
      driver_test_.runtime().RunWithTimeoutOrUntil([&]() { return completed.load(); }, zx::sec(5)));

  async_usb = {};
}

TEST_F(UsbAdbTest, OfflineRxQueuedAndServicedOnline) {
  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  EXPECT_TRUE(client_->StartAdb(std::move(server_end)).ok());
  auto usb_impl =
      fidl::WireClient(std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  constexpr uint32_t kReceiveSize = 128;
  std::atomic<bool> rx_completed = false;
  usb_impl->Receive().ThenExactlyOnce([&](auto& response) {
    ASSERT_TRUE(response.ok());
    ASSERT_TRUE(response.value().is_ok());
    EXPECT_EQ(response.value().value()->data.size(), kReceiveSize);
    rx_completed = true;
  });

  // Wait for the driver to queue the Receive request in its software queue.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t count = 0;
        driver_test_.RunInDriverContext(
            [&](UsbAdbDevice& dev) { count = UsbAdbTestHelper::RxRequestsCount(dev); });
        return count == 1;
      },
      zx::sec(5)));

  // Bring USB online.
  WaitConfigured();
  EnableUsb();

  // Trigger RX complete at the hardware layer.
  driver_test_.RunInEnvironmentTypeContext([&](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).RequestComplete(ZX_OK, kReceiveSize);
  });

  // Wait for the background thread to finish.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil([&]() { return rx_completed.load(); },
                                                           zx::sec(5)));

  usb_impl = {};
}

TEST_F(UsbAdbTest, StartAdbTwiceFails) {
  auto usb_impl = NormalStartAdb();

  // Try to start ADB again with a new channel. It should fail with ZX_ERR_ALREADY_BOUND.
  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  auto result = client_->StartAdb(std::move(server_end));
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value(), ZX_ERR_ALREADY_BOUND);
}

TEST_F(UsbAdbTest, IsConfiguredReflectsDeconfigureState) {
  auto usb_impl = NormalStartAdb();
  driver_test_.RunInEnvironmentTypeContext(
      [&](UsbAdbEnvironment& env) { EXPECT_TRUE(env.fake_dev_->is_configured()); });

  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_hold_cancel(true);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_hold_cancel(true);
  });

  EnsureIfaceBound();
  ASSERT_TRUE(iface_client_.is_valid());
  fidl::Result result = iface_client_->SetConfigured({{
      .configured = false,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kFull,
  }});
  EXPECT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  driver_test_.runtime().RunUntilIdle();

  driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
    EXPECT_EQ(UsbAdbTestHelper::state(dev), State::kStoppingForReconnect);
  });
  driver_test_.RunInEnvironmentTypeContext(
      [&](UsbAdbEnvironment& env) { EXPECT_FALSE(env.fake_dev_->is_configured()); });

  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).ReleaseCancelAll();
    env.fake_dev_->fake_endpoint(kBulkInEp).ReleaseCancelAll();
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_hold_cancel(false);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_hold_cancel(false);
    env.CancelAllUsbRequests();
  });
}

TEST_F(UsbAdbTest, OfflineTxQueuedAndFlushedOnline) {
  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  ASSERT_TRUE(client_->StartAdb(std::move(server_end)).ok());
  auto usb_impl = fidl::WireClient<fadb::UsbAdbImpl>(
      std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Queue a TX request while offline.
  std::vector<uint8_t> test_data(100, 0xAA);
  std::atomic<bool> tx_completed = false;
  usb_impl->QueueTx(fidl::VectorView<uint8_t>::FromExternal(test_data.data(), test_data.size()))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<fadb::UsbAdbImpl::QueueTx>& result) {
        if (result.ok() && result->is_ok()) {
          tx_completed = true;
        }
      });

  // Wait for the driver to receive and queue the request in its software queue.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t count = 0;
        driver_test_.RunInDriverContext(
            [&](UsbAdbDevice& dev) { count = UsbAdbTestHelper::TxPendingReqsCount(dev); });
        return count == 1;
      },
      zx::sec(5)));

  // Verify that nothing has been queued to the hardware yet.
  driver_test_.RunInEnvironmentTypeContext([&](UsbAdbEnvironment& env) {
    EXPECT_EQ(env.fake_dev_->fake_endpoint(kBulkInEp).pending_request_count(), 0u);
  });

  // Now bring USB online.
  WaitConfigured();
  EnableUsb();

  // The driver should now automatically dequeue the request and queue it to bulk IN endpoint.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t pending_endpoints = 0;
        driver_test_.RunInEnvironmentTypeContext([&](UsbAdbEnvironment& env) {
          pending_endpoints = env.fake_dev_->fake_endpoint(kBulkInEp).pending_request_count();
        });
        return pending_endpoints == 1;
      },
      zx::sec(5)));

  // Complete the request at the hardware layer.
  driver_test_.RunInEnvironmentTypeContext([&](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkInEp).RequestComplete(ZX_OK, test_data.size());
  });

  // Wait for the FIDL callback to finish.
  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil([&]() { return tx_completed.load(); },
                                                           zx::sec(5)));
}

TEST_F(UsbAdbTest, UsbFunctionBindingReconfigure) {
  // Establish an initial ADB connection
  auto [client_end1, server_end1] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  auto result1 = client_->StartAdb(std::move(server_end1));
  ASSERT_TRUE(result1.ok());

  // Wait for iface_client_ to become valid
  ASSERT_NO_FATAL_FAILURE(EnsureIfaceBound());

  auto config_result_first = iface_client_->SetConfigured({{
      .configured = true,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  ASSERT_TRUE(config_result_first.is_ok());

  CancelAllUsbRequestsOnDeconfigure();

  auto deconfig_result = iface_client_->SetConfigured({{
      .configured = false,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  ASSERT_TRUE(deconfig_result.is_ok());

  iface_client_ = {};
  ASSERT_NO_FATAL_FAILURE(EnsureIfaceBound());

  auto config_result = iface_client_->SetConfigured({{
      .configured = true,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  ASSERT_TRUE(config_result.is_ok());

  // Let the message loop process the asynchronous unbind handlers
  driver_test_.runtime().RunUntilIdle();

  // Verify that driver is online AND usb_function_binding_ remains bound.
  driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
    EXPECT_EQ(UsbAdbTestHelper::state(dev), State::kOnline);
    EXPECT_TRUE(UsbAdbTestHelper::has_usb_function_binding(dev));
  });

  // Verify the channel to usb_function_binding_ is still live by making a synchronous call.
  auto ping_result = iface_client_->SetConfigured({{
      .configured = true,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  EXPECT_TRUE(ping_result.is_ok());
  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

TEST_F(UsbAdbTest, DisconnectTransitionsToStoppingState) {
  auto usb_impl = NormalStartAdb();
  EventHandler handler;
  handler.expected_statuses_.emplace(fadb::StatusFlags::kOnline);
  ExpectHandleOneEventSafe(usb_impl, handler);

  // Disable cancellation so the fake endpoints hold onto the requests during stopping.
  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_hold_cancel(true);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_hold_cancel(true);
  });

  // Deconfiguring USB transitions driver state to kStoppingForReconnect.
  auto deconfig_result = iface_client_->SetConfigured({{
      .configured = false,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  ASSERT_TRUE(deconfig_result.is_ok());

  WaitForState(State::kStoppingForReconnect);

  // Release cancellation so the driver can finish stopping cleanly.
  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).ReleaseCancelAll();
    env.fake_dev_->fake_endpoint(kBulkInEp).ReleaseCancelAll();
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_hold_cancel(false);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_hold_cancel(false);
    env.CancelAllUsbRequests();
  });

  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

TEST_F(UsbAdbTest, TeardownWhileRequestsPending) {
  auto usb_impl = NormalStartAdb();
  EventHandler handler;
  handler.expected_statuses_.emplace(fadb::StatusFlags::kOnline);
  ExpectHandleOneEventSafe(usb_impl, handler);

  // Disable cancellation so the fake endpoints hold onto the requests.
  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_hold_cancel(true);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_hold_cancel(true);
  });

  // Initiate disconnect/reset while hold is active to set up pending CancelAll requests.
  usb_impl = {};
  iface_client_ = {};

  WaitForState(State::kStoppingForReconnect);

  // Verify the driver is waiting for CancelAll and in stopping state:
  driver_test_.RunInDriverContext([&](UsbAdbDevice& dev) {
    EXPECT_FALSE(UsbAdbTestHelper::CancelAllCompleted(dev));
    EXPECT_FALSE(UsbAdbTestHelper::AllRequestsReturned(dev));
    EXPECT_EQ(UsbAdbTestHelper::state(dev), State::kStoppingForReconnect);
  });

  // Release cancellation so the driver receives the CancelAll callbacks during unbind.
  driver_test_.RunInEnvironmentTypeContext([](UsbAdbEnvironment& env) {
    env.fake_dev_->fake_endpoint(kBulkOutEp).ReleaseCancelAll();
    env.fake_dev_->fake_endpoint(kBulkInEp).ReleaseCancelAll();
    env.fake_dev_->fake_endpoint(kBulkOutEp).set_hold_cancel(false);
    env.fake_dev_->fake_endpoint(kBulkInEp).set_hold_cancel(false);
    env.CancelAllUsbRequests();
  });

  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

TEST_F(UsbAdbTest, InspectStateTransitions) {
  inspect::Inspector inspector;
  driver_test_.RunInDriverContext(
      [&](UsbAdbDevice& dev) { inspector = UsbAdbTestHelper::GetInspector(dev); });
  ASSERT_NO_FATAL_FAILURE(ReadInspect(inspector));
  auto* root_node = this->hierarchy().GetByPath({"usb-adb-function"});
  ASSERT_NE(nullptr, root_node);
  const auto* state_prop = root_node->node().get_property<inspect::StringPropertyValue>("state");
  ASSERT_NE(nullptr, state_prop);
  EXPECT_EQ("kAwaitingUsbConnection", state_prop->value());
  const auto* online_prop = root_node->node().get_property<inspect::BoolPropertyValue>("online");
  ASSERT_NE(nullptr, online_prop);
  EXPECT_FALSE(online_prop->value());

  auto usb_impl = NormalStartAdb();
  driver_test_.RunInDriverContext(
      [&](UsbAdbDevice& dev) { inspector = UsbAdbTestHelper::GetInspector(dev); });
  ASSERT_NO_FATAL_FAILURE(ReadInspect(inspector));
  root_node = this->hierarchy().GetByPath({"usb-adb-function"});
  ASSERT_NE(nullptr, root_node);
  state_prop = root_node->node().get_property<inspect::StringPropertyValue>("state");
  ASSERT_NE(nullptr, state_prop);
  EXPECT_EQ("kOnline", state_prop->value());
  online_prop = root_node->node().get_property<inspect::BoolPropertyValue>("online");
  ASSERT_NE(nullptr, online_prop);
  EXPECT_TRUE(online_prop->value());

  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

TEST_F(UsbAdbTest, StartAdbFailsWhileStopping) {
  driver_test_.RunInDriverContext(
      [](UsbAdbDevice& dev) { UsbAdbTestHelper::set_state(dev, State::kStoppingForUnbind); });

  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  auto result = client_->StartAdb(std::move(server_end));
  EXPECT_TRUE(result.ok());
  EXPECT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value(), ZX_ERR_BAD_STATE);

  driver_test_.RunInDriverContext(
      [](UsbAdbDevice& dev) { UsbAdbTestHelper::set_state(dev, State::kAwaitingUsbConnection); });
  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

TEST_F(UsbAdbTest, OperationsFailWhileStopping) {
  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  auto result = client_->StartAdb(std::move(server_end));
  EXPECT_TRUE(result.ok());

  fidl::WireSyncClient<fadb::UsbAdbImpl> adb_client{std::move(client_end)};

  driver_test_.RunInDriverContext(
      [](UsbAdbDevice& dev) { UsbAdbTestHelper::set_state(dev, State::kStoppingForUnbind); });

  std::vector<uint8_t> data(1);
  auto tx_result =
      adb_client->QueueTx(fidl::VectorView<uint8_t>::FromExternal(data.data(), data.size()));
  EXPECT_TRUE(tx_result.ok());
  EXPECT_TRUE(tx_result->is_error());
  EXPECT_EQ(tx_result->error_value(), ZX_ERR_BAD_STATE);

  auto rx_result = adb_client->Receive();
  EXPECT_TRUE(rx_result.ok());
  EXPECT_TRUE(rx_result->is_error());
  EXPECT_EQ(rx_result->error_value(), ZX_ERR_BAD_STATE);

  driver_test_.RunInDriverContext(
      [](UsbAdbDevice& dev) { UsbAdbTestHelper::set_state(dev, State::kAwaitingUsbConnection); });
  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

TEST_F(UsbAdbTest, EarlyConnectRace) {
  auto [client_end, server_end] = fidl::Endpoints<fadb::UsbAdbImpl>::Create();
  EXPECT_TRUE(client_->StartAdb(std::move(server_end)).ok());

  uint8_t test_data[] = {0x01, 0x02, 0x03};

  fidl::WireClient async_usb(std::move(client_end),
                             fdf::Dispatcher::GetCurrent()->async_dispatcher());

  bool completed = false;
  async_usb->QueueTx(fidl::VectorView<uint8_t>::FromExternal(test_data, sizeof(test_data)))
      .ThenExactlyOnce([&](auto& response) { completed = true; });

  ASSERT_TRUE(driver_test_.runtime().RunWithTimeoutOrUntil(
      [&]() {
        size_t count = 0;
        driver_test_.RunInDriverContext(
            [&](UsbAdbDevice& dev) { count = UsbAdbTestHelper::TxPendingReqsCount(dev); });
        return count == 1;
      },
      zx::sec(5)));

  SafeStopDriver();
}

TEST(UsbAdbStartTest, StartFailsGracefullyWhenConfigureFails) {
  fdf_testing::BackgroundDriverTest<UsbAdbTestConfig> driver_test;
  driver_test.RunInEnvironmentTypeContext(
      [](UsbAdbEnvironment& env) { env.set_configure_status(ZX_ERR_BAD_STATE); });

  // StartDriver must fail with ZX_ERR_BAD_STATE rather than crashing/panicking.
  zx::result<> start_result = driver_test.StartDriver();
  EXPECT_TRUE(start_result.is_error());
  EXPECT_EQ(start_result.status_value(), ZX_ERR_BAD_STATE);
  driver_test.runtime().RunUntilIdle();
}

TEST_F(UsbAdbTest, ReconnectHandlesConfigureFailureGracefully) {
  auto usb_impl = NormalStartAdb();
  EventHandler handler;
  handler.expected_statuses_.emplace(fadb::StatusFlags::kOnline);
  ExpectHandleOneEventSafe(usb_impl, handler);

  // Set the fake device to reject subsequent Configure calls (simulating peripheral
  // stopping/tearing down).
  driver_test_.RunInEnvironmentTypeContext(
      [](UsbAdbEnvironment& env) { env.fake_dev_->set_configure_status(ZX_ERR_BAD_STATE); });

  // Trigger disconnect by deconfiguring USB.
  auto deconfig_result = iface_client_->SetConfigured({{
      .configured = false,
      .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
  }});
  ASSERT_TRUE(deconfig_result.is_ok());

  // Wait for requests to drain and unbind to process.
  driver_test_.runtime().RunUntilIdle();

  // Cancel any remaining USB endpoint requests.
  driver_test_.RunInEnvironmentTypeContext(
      [](UsbAdbEnvironment& env) { env.CancelAllUsbRequests(); });

  // The driver should have attempted to restart USB, encountered ZX_ERR_BAD_STATE,
  // transitioned cleanly to kAwaitingUsbConnection, and logged a warning without panicking.
  // Verify that subsequent StopAdb() completes cleanly (returning zx::ok()) rather than hanging.
  auto stop_res = client_->StopAdb();
  ASSERT_TRUE(stop_res.ok());
  EXPECT_TRUE(stop_res->is_ok());

  ASSERT_NO_FATAL_FAILURE(SafeStopDriver());
}

}  // namespace usb_adb_function
