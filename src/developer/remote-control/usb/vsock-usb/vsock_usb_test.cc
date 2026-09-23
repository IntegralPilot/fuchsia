// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vsock_usb.h"

#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <fidl/fuchsia.hardware.vsockbridge/cpp/wire.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/zx/result.h>
#include <lib/zx/socket.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/status.h>

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <deque>
#include <iterator>
#include <memory>
#include <optional>
#include <queue>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <gtest/gtest.h>
#include <usb-inspect/usb-inspect-test-helper.h>

#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"

// NOLINTBEGIN(misc-use-anonymous-namespace)
// NOLINTBEGIN(readability-convert-member-functions-to-static)
// NOLINTBEGIN(readability-container-data-pointer)

class VsockUsbTestHelper {
 public:
  static zx_status_t UnconfigureEndpoints(VsockUsb& driver) {
    return driver.UnconfigureEndpoints();
  }

  static void Shutdown(VsockUsb& driver, fit::function<void()> callback) {
    driver.Shutdown(std::move(callback));
  }

  static bool Online(VsockUsb& driver) { return driver.Online(); }
};

static constexpr uint8_t kBulkOutEndpoint = 1;
static constexpr uint8_t kBulkInEndpoint = 2;
static constexpr uint8_t kInterfaceNum = 1;
static constexpr size_t kMtu = 1024;

// A fake endpoint that allows for more complex behaviour in responding to completion requests
// by requiring that there be outstanding requests when you attempt to fulfill them.
class FakeEndpoint : public fake_usb_endpoint::FakeEndpoint {
 public:
  ~FakeEndpoint() override {
    fbl::AutoLock _(&lock_);
    EXPECT_TRUE(requests_.empty());
  }

  void Connect(async_dispatcher_t* dispatcher,
               fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server) override {
    fbl::AutoLock _(&lock_);
    binding_ref_.emplace(fidl::BindServer(dispatcher, std::move(server), this));
  }

  void SetEnabled(bool enabled) {
    fbl::AutoLock _(&lock_);
    enabled_ = enabled;
  }

  // QueueRequests: adds requests to a queue, which will be replied to when RequestComplete() is
  // called.
  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override {
    FDF_LOG(DEBUG, "QueueRequests");
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> binding;
    std::vector<fuchsia_hardware_usb_request::Request> reqs_to_cancel;
    {
      fbl::AutoLock _(&lock_);
      if (!enabled_) {
        FDF_LOG(WARNING, "QueueRequests: endpoint is disabled, immediately canceling %zu requests",
                request.req().size());
        reqs_to_cancel.swap(request.req());
        binding = binding_ref_;
      } else {
        // Add request to queue.
        requests_.insert(requests_.end(), std::make_move_iterator(request.req().begin()),
                         std::make_move_iterator(request.req().end()));
      }
    }
    if (!reqs_to_cancel.empty()) {
      if (!binding.has_value()) {
        ADD_FAILURE()
            << "QueueRequests: endpoint disabled with requests to cancel but no active binding";
        return;
      }
      std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
      completions.reserve(reqs_to_cancel.size());
      for (auto& req : reqs_to_cancel) {
        fuchsia_hardware_usb_endpoint::Completion completion;
        completion.request(std::move(req));
        completion.status(ZX_ERR_CANCELED);
        completion.transfer_size(0);
        completions.push_back(std::move(completion));
      }
      auto event_result = fidl::SendEvent(*binding)->OnCompletion(std::move(completions));
      EXPECT_TRUE(event_result.is_ok() || event_result.error_value().status() == ZX_ERR_PEER_CLOSED)
          << "SendEvent failed: " << zx_status_get_string(event_result.error_value().status());
    }
  }

  void CancelAll(CancelAllCompleter::Sync& completer) override {
    std::deque<fuchsia_hardware_usb_request::Request> reqs_to_cancel;
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> binding;
    {
      fbl::AutoLock _(&lock_);
      reqs_to_cancel.swap(requests_);
      binding = binding_ref_;
    }
    if (!reqs_to_cancel.empty()) {
      if (!binding.has_value()) {
        ADD_FAILURE() << "CancelAll: endpoint has requests to cancel but no active binding";
        completer.Reply(fit::ok());
        return;
      }
      std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
      completions.reserve(reqs_to_cancel.size());
      for (auto& req : reqs_to_cancel) {
        fuchsia_hardware_usb_endpoint::Completion completion;
        completion.request(std::move(req));
        completion.status(ZX_ERR_CANCELED);
        completion.transfer_size(0);
        completions.push_back(std::move(completion));
      }
      auto event_result = fidl::SendEvent(*binding)->OnCompletion(std::move(completions));
      EXPECT_TRUE(event_result.is_ok() || event_result.error_value().status() == ZX_ERR_PEER_CLOSED)
          << "SendEvent failed: " << zx_status_get_string(event_result.error_value().status());
    }
    completer.Reply(fit::ok());
  }

  // Non-blocking atomic request poll for assertions.
  std::optional<fuchsia_hardware_usb_request::Request> PopNextRequest() {
    fbl::AutoLock _(&lock_);
    if (requests_.empty()) {
      return std::nullopt;
    }
    auto next_request = std::move(requests_.front());
    requests_.pop_front();
    return next_request;
  }

  void SendRequestComplete(fuchsia_hardware_usb_request::Request request, zx_status_t status,
                           size_t actual) {
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> binding;
    {
      fbl::AutoLock _(&lock_);
      binding = binding_ref_;
    }
    if (!binding.has_value()) {
      ADD_FAILURE() << "SendRequestComplete called without active binding";
      return;
    }
    fuchsia_hardware_usb_endpoint::Completion completion;
    completion.request(std::move(request));
    completion.status(status);
    completion.transfer_size(actual);
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    completions.push_back(std::move(completion));
    auto event_result = fidl::SendEvent(*binding)->OnCompletion(std::move(completions));
    EXPECT_TRUE(event_result.is_ok() || event_result.error_value().status() == ZX_ERR_PEER_CLOSED)
        << "SendEvent failed: " << zx_status_get_string(event_result.error_value().status());
  }

  // RegisterVmos: stores the vmo mapping
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) override {
    fbl::AutoLock lock(&lock_);
    std::vector<fuchsia_hardware_usb_endpoint::VmoHandle> ret;
    for (const auto& vmo_id : request.vmo_ids()) {
      if (!vmo_id.id().has_value() || !vmo_id.size().has_value()) {
        ADD_FAILURE() << "RegisterVmos received VmoId without id or size";
        continue;
      }
      zx::vmo vmo;
      auto status = zx::vmo::create(*vmo_id.size(), 0, &vmo);
      if (status != ZX_OK) {
        ADD_FAILURE() << "RegisterVmos failed to create VMO for id " << *vmo_id.id() << ": "
                      << zx_status_get_string(status);
        continue;
      }
      zx::vmo dup_vmo;
      zx_status_t dup_status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_vmo);
      EXPECT_EQ(dup_status, ZX_OK);
      if (dup_status != ZX_OK) {
        ADD_FAILURE() << "RegisterVmos failed to duplicate VMO: "
                      << zx_status_get_string(dup_status);
        continue;
      }
      vmos_.emplace(*vmo_id.id(), std::move(dup_vmo));
      ret.emplace_back(std::move(
          fuchsia_hardware_usb_endpoint::VmoHandle().id(*vmo_id.id()).vmo(std::move(vmo))));
    }
    completer.Reply(std::move(ret));
  }
  // UnregisterVmos: stores the vmo mapping
  void UnregisterVmos(UnregisterVmosRequest& request,
                      UnregisterVmosCompleter::Sync& completer) override {
    fbl::AutoLock lock(&lock_);
    for (const auto& vmo_id : request.vmo_ids()) {
      vmos_.erase(vmo_id);
    }
    completer.Reply({{}, {}});
  }

  [[nodiscard]] bool WithVmo(uint64_t vmo_id, fit::function<void(zx::vmo&)> cb) {
    ZX_ASSERT(cb != nullptr);
    fbl::AutoLock lock(&lock_);
    auto vmo = vmos_.find(vmo_id);
    if (vmo == vmos_.end()) {
      ADD_FAILURE() << "VMO ID " << vmo_id << " not found in registered VMOs";
      return false;
    }
    cb(vmo->second);
    return true;
  }

 private:
  fbl::Mutex lock_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> binding_ref_
      __TA_GUARDED(lock_);
  bool enabled_ __TA_GUARDED(lock_) = true;
  std::deque<fuchsia_hardware_usb_request::Request> requests_ __TA_GUARDED(lock_);
  std::unordered_map<uint64_t, zx::vmo> vmos_ __TA_GUARDED(lock_);
};

class TestCallback : public fidl::WireServer<fuchsia_hardware_vsockbridge::Callback> {
 public:
  TestCallback(async_dispatcher_t* dispatcher,
               fidl::ServerEnd<fuchsia_hardware_vsockbridge::Callback> server_end,
               size_t expected_calls)
      : expected_calls_(expected_calls) {
    binding_.emplace(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  }

  ~TestCallback() override {
    // Unbind and destroy the FIDL server binding first so no further messages
    // can arrive or be dispatched while checking test expectations.
    binding_.reset();

    size_t actual_calls;
    {
      fbl::AutoLock _(&lock_);
      actual_calls = actual_calls_;
    }
    FDF_LOG(DEBUG, "Destroying TestCallback %zu==%zu", expected_calls_, actual_calls);
    EXPECT_EQ(actual_calls, expected_calls_);
  }

  void NewLink(::fuchsia_hardware_vsockbridge::wire::CallbackNewLinkRequest* request,
               NewLinkCompleter::Sync& completer) override {
    size_t calls;
    {
      fbl::AutoLock _(&lock_);
      actual_calls_++;
      calls = actual_calls_;
      sockets_.push_back(std::move(request->socket));
    }
    FDF_LOG(DEBUG, "calling callback %zu", calls);
    completer.Reply();
  }

  // Non-blocking atomic socket poll.
  std::optional<zx::socket> PopSocket() {
    fbl::AutoLock _(&lock_);
    if (sockets_.empty()) {
      return std::nullopt;
    }
    auto sock = std::move(sockets_.front());
    sockets_.pop_front();
    return sock;
  }

  // Waits until a socket is received by driving the runtime dispatcher.
  zx::socket WaitForSocket(fdf_testing::DriverRuntime& runtime) {
    std::optional<zx::socket> sock;
    runtime.RunUntil([&]() {
      sock = PopSocket();
      return sock.has_value();
    });
    return std::move(*sock);
  }

 private:
  fbl::Mutex lock_;
  size_t expected_calls_;
  size_t actual_calls_ __TA_GUARDED(lock_) = 0;
  std::deque<zx::socket> sockets_ __TA_GUARDED(lock_);
  std::optional<fidl::ServerBinding<fuchsia_hardware_vsockbridge::Callback>> binding_;

  TestCallback(const TestCallback&) = delete;
  TestCallback& operator=(const TestCallback&) = delete;
};

class FakeUsb
    : public fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction,
                                                    FakeEndpoint> {
 public:
  using Base = fake_usb_endpoint::FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction,
                                                      FakeEndpoint>;
  using Base::Base;

  ~FakeUsb() override {
    fbl::AutoLock _(&lock_);
    EXPECT_TRUE(expect_configure_ep_.empty());
    EXPECT_TRUE(expect_disable_ep_.empty());
  }

  void Configure(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure>& request,
      fidl::internal::NaturalCompleter<fuchsia_hardware_usb_function::UsbFunction::Configure>::Sync&
          completer) override {
    {
      fbl::AutoLock _(&lock_);
      interface_ = std::move(request.iface());
    }
    completer.Reply(fit::ok());
  }

  void AllocResources(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::AllocResources>::Sync& completer) override {
    if (request.endpoints().size() != 2u || request.interface_count() != 1u ||
        request.strings().size() != 1u) {
      ADD_FAILURE() << "AllocResources received unexpected parameters: endpoints="
                    << request.endpoints().size() << ", interfaces=" << request.interface_count()
                    << ", strings=" << request.strings().size();
      completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    fuchsia_hardware_usb_function::UsbFunctionAllocResourcesResponse response;
    response.interface_nums() = {kInterfaceNum};
    response.endpoint_addrs() = {kBulkOutEndpoint, kBulkInEndpoint};
    response.string_indices() = {1};
    for (size_t i = 0; i < 2; i++) {
      fidl::ServerEnd ep = std::move(request.endpoints()[i].endpoint());
      fake_endpoint(response.endpoint_addrs()[i]).Connect(dispatcher(), std::move(ep));
    }
    completer.Reply(fit::ok(std::move(response)));
  }

  void ConfigureEndpoint(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>::Sync& completer)
      override {
    fake_endpoint(request.endpoint_address()).SetEnabled(true);
    completer.Reply(fit::ok());
    fbl::AutoLock _(&lock_);
    if (expect_configure_ep_.empty()) {
      ADD_FAILURE() << "received ConfigureEndpoint "
                    << static_cast<uint32_t>(request.endpoint_address()) << " without expectation";
      return;
    }
    EXPECT_EQ(request.endpoint_address(), expect_configure_ep_.front());
    expect_configure_ep_.pop();
  }

  void DisableEndpoint(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>::Sync& completer) override {
    bool has_expectation = false;
    std::optional<zx_status_t> status;
    {
      fbl::AutoLock _(&lock_);
      status = disable_ep_status_;
      if (expect_disable_ep_.empty()) {
        ADD_FAILURE() << "received DisableEndpoint "
                      << static_cast<uint32_t>(request.endpoint_address())
                      << " without expectation";
      } else {
        has_expectation = true;
        EXPECT_EQ(request.endpoint_address(), expect_disable_ep_.front());
        expect_disable_ep_.pop();
      }
    }
    if (!has_expectation) {
      completer.Reply(fit::error(ZX_ERR_BAD_STATE));
      return;
    }
    if (status.has_value() && *status != ZX_OK) {
      completer.Reply(fit::error(*status));
      return;
    }

    auto& ep = fake_endpoint(request.endpoint_address());
    ep.SetEnabled(false);
    completer.Reply(fit::ok());
  }

  void ExpectConfigureEndpoint(uint8_t endpoint_address) {
    fbl::AutoLock _(&lock_);
    expect_configure_ep_.push(endpoint_address);
  }

  void ExpectDisableEndpoint(uint8_t endpoint_address) {
    fbl::AutoLock _(&lock_);
    expect_disable_ep_.push(endpoint_address);
  }

  void set_disable_ep_status(std::optional<zx_status_t> status) {
    fbl::AutoLock _(&lock_);
    ZX_ASSERT_MSG(!status.has_value() || *status != ZX_OK,
                  "disable_ep_status cannot be set to ZX_OK");
    disable_ep_status_ = status;
  }

  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> TakeInterface() {
    fbl::AutoLock _(&lock_);
    return std::move(interface_);
  }

 private:
  fbl::Mutex lock_;
  std::optional<zx_status_t> disable_ep_status_ __TA_GUARDED(lock_);
  std::queue<uint8_t> expect_configure_ep_ __TA_GUARDED(lock_);
  std::queue<uint8_t> expect_disable_ep_ __TA_GUARDED(lock_);
  fidl::ClientEnd<fuchsia_hardware_usb_function::UsbFunctionInterface> interface_
      __TA_GUARDED(lock_);
};

class VsockUsbEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    device_server_.Initialize("default");
    if (auto res = device_server_.Serve(dispatcher_, &to_driver_vfs); res != ZX_OK) {
      return zx::error(res);
    }
    fake_usb_ = std::make_unique<FakeUsb>(dispatcher_);
    fuchsia_hardware_usb_function::UsbFunctionService::InstanceHandler handler({
        .device = usb_function_bindings_.CreateHandler(
            fake_usb_.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
            fidl::kIgnoreBindingClosure),
    });
    auto result = to_driver_vfs.AddService<fuchsia_hardware_usb_function::UsbFunctionService>(
        std::move(handler));
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  async_dispatcher_t* dispatcher_;
  compat::DeviceServer device_server_;
  std::unique_ptr<FakeUsb> fake_usb_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_function::UsbFunction> usb_function_bindings_;
};

class VsockUsbTestConfig final {
 public:
  using DriverType = VsockUsb;
  using EnvironmentType = VsockUsbEnvironment;
};

class VsockUsbTest : public ::testing::Test {
 public:
  [[nodiscard]] zx::socket WaitForSocket(TestCallback& callback) {
    return callback.WaitForSocket(driver_test().runtime());
  }

  [[nodiscard]] std::optional<fuchsia_hardware_usb_request::Request> WaitForRequestOn(
      uint8_t endpoint) {
    FDF_LOG(DEBUG, "Waiting for request on endpoint %d", endpoint);
    std::optional<fuchsia_hardware_usb_request::Request> request;
    driver_test().runtime().RunUntil([&]() {
      driver_test().RunInEnvironmentTypeContext([&](VsockUsbEnvironment& env) {
        auto& ep = env.fake_usb_->fake_endpoint(endpoint);
        request = ep.PopNextRequest();
      });
      return request.has_value();
    });
    return request;
  }

  [[nodiscard]] bool CompleteMockUsbOutRequest(fuchsia_hardware_usb_request::Request request,
                                               uint8_t endpoint, const uint8_t* tx, size_t size,
                                               zx_status_t status) {
    ZX_ASSERT_MSG(tx != nullptr || size == 0, "tx must be non-null when size > 0");
    bool ok = true;
    driver_test().RunInEnvironmentTypeContext([&](VsockUsbEnvironment& env) {
      auto& out_ep = env.fake_usb_->fake_endpoint(endpoint);
      auto& data = request.data();
      if (!data.has_value() || data->size() != 1u) {
        ADD_FAILURE() << "Request data buffer missing or invalid";
        out_ep.SendRequestComplete(std::move(request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      auto& buffer = data->front().buffer();
      if (!buffer.has_value() ||
          buffer->Which() != fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) {
        ADD_FAILURE() << "Buffer is not kVmoId";
        out_ep.SendRequestComplete(std::move(request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      if (!buffer->vmo_id().has_value()) {
        ADD_FAILURE() << "Buffer vmo_id is missing";
        out_ep.SendRequestComplete(std::move(request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      auto vmo_id = buffer->vmo_id().value();
      if (tx != nullptr && size > 0) {
        size_t offset = data->front().offset().value_or(0);
        if (!out_ep.WithVmo(vmo_id, [&](zx::vmo& vmo) {
              zx_status_t write_status = vmo.write(tx, offset, size);
              if (write_status != ZX_OK) {
                ADD_FAILURE() << "VMO write failed: " << zx_status_get_string(write_status);
                ok = false;
              }
            })) {
          out_ep.SendRequestComplete(std::move(request), ZX_ERR_INTERNAL, 0);
          ok = false;
          return;
        }
      }
      if (!ok) {
        out_ep.SendRequestComplete(std::move(request), ZX_ERR_INTERNAL, 0);
        return;
      }
      data->front().size(size);
      out_ep.SendRequestComplete(std::move(request), status, size);
    });
    return ok;
  }

  [[nodiscard]] bool ExecuteMockUsbOutTransaction(uint8_t endpoint, const uint8_t* tx,
                                                  size_t size) {
    ZX_ASSERT_MSG(tx != nullptr || size == 0, "tx must be non-null when size > 0");
    auto request = WaitForRequestOn(endpoint);
    if (!request.has_value()) {
      return false;
    }
    return CompleteMockUsbOutRequest(std::move(*request), endpoint, tx, size, ZX_OK);
  }

  [[nodiscard]] bool ExecuteMockUsbInTransaction(uint8_t endpoint, std::vector<uint8_t>* out_data) {
    ZX_ASSERT(out_data != nullptr);
    auto request = WaitForRequestOn(endpoint);
    if (!request.has_value()) {
      return false;
    }
    bool ok = true;
    driver_test().RunInEnvironmentTypeContext([&](VsockUsbEnvironment& env) {
      auto& in_ep = env.fake_usb_->fake_endpoint(endpoint);
      auto& data = request->data();
      if (!data.has_value() || data->size() != 1u) {
        ADD_FAILURE() << "Request data buffer missing or invalid";
        in_ep.SendRequestComplete(std::move(*request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      auto& buffer = data->front().buffer();
      if (!buffer.has_value() ||
          buffer->Which() != fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) {
        ADD_FAILURE() << "Buffer is not kVmoId";
        in_ep.SendRequestComplete(std::move(*request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      if (!buffer->vmo_id().has_value()) {
        ADD_FAILURE() << "Buffer vmo_id is missing";
        in_ep.SendRequestComplete(std::move(*request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      if (!data->front().size().has_value()) {
        ADD_FAILURE() << "Buffer region size is missing";
        in_ep.SendRequestComplete(std::move(*request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      uint64_t vmo_id = buffer->vmo_id().value();
      out_data->resize(data->front().size().value());
      EXPECT_EQ(request->short_().value_or(false), out_data->size() < kMtu);
      size_t offset = data->front().offset().value_or(0);
      if (!in_ep.WithVmo(vmo_id, [&](zx::vmo& vmo) {
            zx_status_t read_status = vmo.read(out_data->data(), offset, out_data->size());
            if (read_status != ZX_OK) {
              ADD_FAILURE() << "VMO read failed: " << zx_status_get_string(read_status);
              ok = false;
            }
          })) {
        in_ep.SendRequestComplete(std::move(*request), ZX_ERR_INTERNAL, 0);
        ok = false;
        return;
      }
      if (!ok) {
        in_ep.SendRequestComplete(std::move(*request), ZX_ERR_INTERNAL, 0);
        return;
      }
      in_ep.SendRequestComplete(std::move(*request), ZX_OK, out_data->size());
    });
    return ok;
  }

  [[nodiscard]] bool SendTx(const uint8_t* tx, size_t size) {
    ZX_ASSERT_MSG(tx != nullptr || size == 0, "tx must be non-null when size > 0");
    FDF_LOG(DEBUG, "SendTx(%zu)", size);
    return ExecuteMockUsbOutTransaction(kBulkOutEndpoint, tx, size);
  }

  [[nodiscard]] std::optional<std::vector<uint8_t>> GetRx() {
    std::vector<uint8_t> ret;
    if (!ExecuteMockUsbInTransaction(kBulkInEndpoint, &ret)) {
      return std::nullopt;
    }
    return ret;
  }

  [[nodiscard]] bool GetRxConcatExpect(const uint8_t* data, size_t len) {
    ZX_ASSERT_MSG(data != nullptr || len == 0, "data must be non-null when len > 0");
    FDF_LOG(DEBUG, "GetRxConcatExpect(%zu)", len);
    while (len != 0) {
      auto got = GetRx();
      if (!got.has_value()) {
        FDF_LOG(ERROR, "No value returned from GetRx");
        return false;
      }
      if (got->empty()) {
        FDF_LOG(ERROR, "Received empty packet while expecting %zu bytes", len);
        return false;
      }
      if (got->size() > len) {
        FDF_LOG(ERROR, "returned size %zu was greater than expected %zu", got->size(), len);
        return false;
      }
      if (!std::equal(got->begin(), got->end(), data)) {
        FDF_LOG(ERROR, "returned data did not match expectation");
        return false;
      }
      len -= got->size();
      data += got->size();
    }
    return true;
  }

  [[nodiscard]] bool SocketReadExpect(zx::socket* socket, const uint8_t* data, size_t len) {
    ZX_ASSERT(socket != nullptr);
    ZX_ASSERT_MSG(data != nullptr || len == 0, "data must be non-null when len > 0");
    FDF_LOG(DEBUG, "SocketReadExpect(%zu)", len);
    std::vector<uint8_t> buf(len, 0);

    while (len > 0) {
      FDF_LOG(DEBUG, "reading loop iteration, need %zu bytes still", len);
      zx_signals_t pending;
      size_t actual;
      if (socket->wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED, zx::time::infinite(),
                           &pending) != ZX_OK) {
        return false;
      }
      if ((pending & ZX_SOCKET_READABLE) == 0) {
        return false;
      }
      if (socket->read(0, buf.data(), len, &actual) != ZX_OK) {
        return false;
      }
      if (actual == 0) {
        FDF_LOG(ERROR, "Socket read 0 bytes while %zu bytes expected", len);
        return false;
      }
      if (!std::equal(buf.begin(), buf.begin() + static_cast<ssize_t>(actual), data)) {
        return false;
      }
      len -= actual;
      data += actual;
    }
    return true;
  }

  [[nodiscard]] bool SocketWriteAll(zx::socket* socket, const uint8_t* data, size_t len) {
    ZX_ASSERT(socket != nullptr);
    ZX_ASSERT_MSG(data != nullptr || len == 0, "data must be non-null when len > 0");
    FDF_LOG(DEBUG, "SocketWriteAll(%zu)", len);
    while (len > 0) {
      zx_signals_t pending;
      size_t actual;
      zx_status_t res = socket->wait_one(ZX_SOCKET_WRITABLE | ZX_SOCKET_PEER_CLOSED,
                                         zx::time::infinite(), &pending);
      if (res != ZX_OK) {
        FDF_LOG(ERROR, "error while waiting on socket: %d", res);
        return false;
      }
      if ((pending & ZX_SOCKET_WRITABLE) == 0) {
        FDF_LOG(ERROR, "socket not writeable (%x)", pending);
        return false;
      }
      res = socket->write(0, data, len, &actual);
      if (res != ZX_OK) {
        FDF_LOG(ERROR, "error while writing to socket: %d", res);
        return false;
      }
      if (actual == 0) {
        FDF_LOG(ERROR, "Socket wrote 0 bytes while %zu bytes remaining", len);
        return false;
      }
      FDF_LOG(DEBUG, "wrote %zu bytes to socket", actual);
      len -= actual;
      data += actual;
    }
    return true;
  }

  void SetUp() override {
    ASSERT_TRUE(driver_test().StartDriver().is_ok());
    driver_test().RunInEnvironmentTypeContext([this](VsockUsbEnvironment& env) {
      function_client_.Bind(env.fake_usb_->TakeInterface());
    });
    auto device = driver_test().Connect<fuchsia_hardware_vsockbridge::UsbService::Device>();
    ASSERT_TRUE(device.is_ok());
    client_.Bind(std::move(device.value()));
    driver_test().RunInDriverContext([](VsockUsb& driver) {
      EXPECT_EQ(driver.BulkInAddress(), kBulkInEndpoint);
      EXPECT_EQ(driver.BulkOutAddress(), kBulkOutEndpoint);
    });
    driver_stopped_ = false;
  }

  void TearDown() override {
    FDF_LOG(DEBUG, "TearDown start");

    if (!driver_stopped_) {
      zx::result<> result = driver_test().StopDriver();
      ASSERT_TRUE(result.is_ok());
    }
    driver_test().runtime().RunUntilIdle();
    FDF_LOG(DEBUG, "TearDown finished");
  }

  void ConfigureDevice() {
    ExpectConfigureEndpoints();
    fidl::Result result = function_client_->SetConfigured({{
        .configured = true,
        .speed = fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  void UnconfigureDevice() {
    FDF_LOG(DEBUG, "Unconfiguring device");
    ExpectDisableEndpoints();
    fidl::Result result = function_client_->SetConfigured({{
        .configured = false,
    }});
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  }

  void ResetDevice(std::unique_ptr<TestCallback>& callback) {
    FDF_LOG(DEBUG, "Resetting device via UnconfigureDevice() and ConfigureDevice()");
    UnconfigureDevice();
    driver_test().runtime().RunUntilIdle();
    callback = SetupCallback(1);
    ConfigureDevice();
  }

  void ExpectConfigureEndpoints() {
    driver_test().RunInEnvironmentTypeContext([](VsockUsbEnvironment& env) {
      env.fake_usb_->ExpectConfigureEndpoint(kBulkInEndpoint);
      env.fake_usb_->ExpectConfigureEndpoint(kBulkOutEndpoint);
    });
  }

  void ExpectDisableEndpoints() {
    driver_test().RunInEnvironmentTypeContext([](VsockUsbEnvironment& env) {
      env.fake_usb_->ExpectDisableEndpoint(kBulkInEndpoint);
      env.fake_usb_->ExpectDisableEndpoint(kBulkOutEndpoint);
    });
  }

  std::unique_ptr<TestCallback> SetupCallback(size_t expected_calls) {
    auto callback_obj = SetTestCallback(expected_calls);
    EXPECT_TRUE(callback_obj);
    driver_test().runtime().RunUntilIdle();
    return callback_obj;
  }

  std::unique_ptr<TestCallback> SetTestCallback(size_t expected_calls) const {
    auto dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_vsockbridge::Callback>();
    if (!endpoints.is_ok()) {
      return nullptr;
    }
    auto ret =
        std::make_unique<TestCallback>(dispatcher, std::move(endpoints->server), expected_calls);
    if (!client_->SetCallback(std::move(endpoints->client)).ok()) {
      return nullptr;
    }
    return ret;
  }

  fdf_testing::BackgroundDriverTest<VsockUsbTestConfig>& driver_test() { return driver_test_; }
  fdf_testing::BackgroundDriverTest<VsockUsbTestConfig> driver_test_;
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunctionInterface> function_client_;
  fidl::WireSyncClient<fuchsia_hardware_vsockbridge::Usb> client_;
  bool driver_stopped_ = false;
};

// Tests that the driver initializes and starts up cleanly in the driver test realm.
TEST_F(VsockUsbTest, Startup) { FDF_LOG(DEBUG, "startup"); }

TEST_F(VsockUsbTest, ConfigureAndUnconfigure) {
  ConfigureDevice();
  UnconfigureDevice();
}

TEST_F(VsockUsbTest, SocketGet) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket sock = WaitForSocket(*callback);
  ASSERT_TRUE(sock.is_valid());
  FDF_LOG(DEBUG, "Callback setup");
  UnconfigureDevice();
}

TEST_F(VsockUsbTest, DataFromTarget) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  std::string_view test_data =
      "A basket of biscuits, a basket of mixed biscuits and a biscuit mixer.";
  ASSERT_TRUE(SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(test_data.data()),
                             test_data.size()));

  ASSERT_TRUE(
      GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data.data()), test_data.size()));

  std::string_view test_data_b = "Aluminum, linoleum, magnesium, petroleum.";
  ASSERT_TRUE(SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(test_data_b.data()),
                             test_data_b.size()));
  ASSERT_TRUE(
      GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_b.data()), test_data_b.size()));
  UnconfigureDevice();
}

TEST_F(VsockUsbTest, DataFromHost) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  std::string_view test_data =
      "A basket of biscuits, a basket of mixed biscuits and a biscuit mixer.";
  ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data.data()), test_data.size()));
  ASSERT_TRUE(SocketReadExpect(&socket, reinterpret_cast<const uint8_t*>(test_data.data()),
                               test_data.size()));

  std::string_view test_data_b = "Aluminum, linoleum, magnesium, petroleum.";
  ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data_b.data()), test_data_b.size()));
  ASSERT_TRUE(SocketReadExpect(&socket, reinterpret_cast<const uint8_t*>(test_data_b.data()),
                               test_data_b.size()));
  UnconfigureDevice();
}

TEST_F(VsockUsbTest, DataFromHostQueuedDatagrams) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  std::vector<std::vector<uint8_t>> test_packets;
  for (size_t i = 0; i < 50; ++i) {
    std::vector<uint8_t> pkt(256, static_cast<uint8_t>(i));
    ASSERT_TRUE(SendTx(pkt.data(), pkt.size()));
    test_packets.push_back(std::move(pkt));
  }

  driver_test().runtime().RunUntilIdle();

  for (size_t i = 0; i < test_packets.size(); ++i) {
    std::vector<uint8_t> buf(test_packets[i].size());
    size_t actual = 0;
    while (true) {
      zx_status_t status = socket.read(0, buf.data(), buf.size(), &actual);
      if (status == ZX_OK) {
        break;
      }
      ASSERT_EQ(status, ZX_ERR_SHOULD_WAIT);
      driver_test().runtime().RunUntilIdle();
    }
    ASSERT_EQ(actual, test_packets[i].size());
    ASSERT_EQ(buf, test_packets[i]);
  }

  UnconfigureDevice();
}

TEST_F(VsockUsbTest, Reset) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket0 = WaitForSocket(*callback);
  ASSERT_TRUE(socket0.is_valid());

  std::string_view test_data =
      "A basket of biscuits, a basket of mixed biscuits and a biscuit mixer.";
  ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data.data()), test_data.size()));
  ASSERT_TRUE(SocketReadExpect(&socket0, reinterpret_cast<const uint8_t*>(test_data.data()),
                               test_data.size()));

  std::string_view test_data_b = "Aluminum, linoleum, magnesium, petroleum.";
  ASSERT_TRUE(SocketWriteAll(&socket0, reinterpret_cast<const uint8_t*>(test_data_b.data()),
                             test_data_b.size()));
  ASSERT_TRUE(
      GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_b.data()), test_data_b.size()));

  ResetDevice(callback);
  zx::socket socket1 = WaitForSocket(*callback);
  ASSERT_TRUE(socket1.is_valid());

  zx_signals_t pending;
  ASSERT_EQ(socket0.wait_one(ZX_SOCKET_PEER_CLOSED, zx::time::infinite(), &pending), ZX_OK);
  ASSERT_NE(pending & ZX_SOCKET_PEER_CLOSED, 0u);

  std::string_view test_data_c = "Around the rugged rocks the ragged rascals ran.";
  ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data_c.data()), test_data_c.size()));
  ASSERT_TRUE(SocketReadExpect(&socket1, reinterpret_cast<const uint8_t*>(test_data_c.data()),
                               test_data_c.size()));

  std::string_view test_data_d = "A proper copper coffee pot.";
  ASSERT_TRUE(SocketWriteAll(&socket1, reinterpret_cast<const uint8_t*>(test_data_d.data()),
                             test_data_d.size()));
  ASSERT_TRUE(
      GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_d.data()), test_data_d.size()));
  UnconfigureDevice();
}

TEST_F(VsockUsbTest, ResetMoreData) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket0 = WaitForSocket(*callback);
  ASSERT_TRUE(socket0.is_valid());

  std::string_view test_data_a =
      "A basket of biscuits, a basket of mixed biscuits and a biscuit mixer.";
  std::string_view test_data_b = "Aluminum, linoleum, magnesium, petroleum.";
  std::string_view test_data_c = "Around the rugged rocks the ragged rascals ran.";
  std::string_view test_data_d = "A proper copper coffee pot.";

  for (int i = 0; i < 50; i++) {
    ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data_a.data()), test_data_a.size()));
    ASSERT_TRUE(SocketReadExpect(&socket0, reinterpret_cast<const uint8_t*>(test_data_a.data()),
                                 test_data_a.size()));

    ASSERT_TRUE(SocketWriteAll(&socket0, reinterpret_cast<const uint8_t*>(test_data_b.data()),
                               test_data_b.size()));
    ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_b.data()),
                                  test_data_b.size()));

    ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data_c.data()), test_data_c.size()));
    ASSERT_TRUE(SocketReadExpect(&socket0, reinterpret_cast<const uint8_t*>(test_data_c.data()),
                                 test_data_c.size()));

    ASSERT_TRUE(SocketWriteAll(&socket0, reinterpret_cast<const uint8_t*>(test_data_d.data()),
                               test_data_d.size()));
    ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_d.data()),
                                  test_data_d.size()));
  }
  ResetDevice(callback);
  zx::socket socket1 = WaitForSocket(*callback);
  ASSERT_TRUE(socket1.is_valid());

  zx_signals_t pending;
  ASSERT_EQ(socket0.wait_one(ZX_SOCKET_PEER_CLOSED, zx::time::infinite(), &pending), ZX_OK);
  ASSERT_NE(pending & ZX_SOCKET_PEER_CLOSED, 0u);

  for (int i = 0; i < 50; i++) {
    ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data_a.data()), test_data_a.size()));
    ASSERT_TRUE(SocketReadExpect(&socket1, reinterpret_cast<const uint8_t*>(test_data_a.data()),
                                 test_data_a.size()));

    ASSERT_TRUE(SocketWriteAll(&socket1, reinterpret_cast<const uint8_t*>(test_data_b.data()),
                               test_data_b.size()));
    ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_b.data()),
                                  test_data_b.size()));

    ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data_c.data()), test_data_c.size()));
    ASSERT_TRUE(SocketReadExpect(&socket1, reinterpret_cast<const uint8_t*>(test_data_c.data()),
                                 test_data_c.size()));

    ASSERT_TRUE(SocketWriteAll(&socket1, reinterpret_cast<const uint8_t*>(test_data_d.data()),
                               test_data_d.size()));
    ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_data_d.data()),
                                  test_data_d.size()));
  }
  UnconfigureDevice();
}

TEST_F(VsockUsbTest, Inspect) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  std::string_view host_to_device_data = "Host to Device (RX for driver)";
  std::string_view device_to_host_data = "Device to Host (TX for driver)";

  ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(host_to_device_data.data()),
                     host_to_device_data.size()));
  ASSERT_TRUE(SocketReadExpect(&socket,
                               reinterpret_cast<const uint8_t*>(host_to_device_data.data()),
                               host_to_device_data.size()));

  ASSERT_TRUE(SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(device_to_host_data.data()),
                             device_to_host_data.size()));
  ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(device_to_host_data.data()),
                                device_to_host_data.size()));

  driver_test().runtime().RunUntil([&]() {
    bool has_pending = true;
    driver_test().RunInDriverContext(
        [&has_pending](VsockUsb& driver) { has_pending = driver.HasPendingTxRequests(); });
    return !has_pending;
  });

  driver_test().RunInDriverContext([tx_size = device_to_host_data.size(),
                                    rx_size = host_to_device_data.size()](VsockUsb& driver) {
    driver.GetThroughputTrackerForTesting().MeasureForTesting(zx::sec(1));

    auto hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());

    auto* vsock_node = hierarchy.GetByPath({"vsock-usb"});
    ASSERT_TRUE(vsock_node != nullptr);

    const auto* state_prop = vsock_node->node().get_property<inspect::StringPropertyValue>("state");
    ASSERT_TRUE(state_prop != nullptr);
    EXPECT_EQ(state_prop->value(), "Running");

    const auto* online_prop = vsock_node->node().get_property<inspect::BoolPropertyValue>("online");
    ASSERT_TRUE(online_prop != nullptr);
    EXPECT_TRUE(online_prop->value());

    auto* bulk_in = hierarchy.GetByPath({"vsock-usb", "bulk_in"});

    ASSERT_TRUE(bulk_in != nullptr);
    auto err_in = usb_inspect::VerifyEndpointInspect(bulk_in, tx_size, std::nullopt, 0,
                                                     std::nullopt, tx_size);
    EXPECT_TRUE(err_in.is_ok()) << err_in.error_value();

    auto* bulk_out = hierarchy.GetByPath({"vsock-usb", "bulk_out"});
    ASSERT_TRUE(bulk_out != nullptr);
    auto err_out = usb_inspect::VerifyEndpointInspect(bulk_out, std::nullopt, rx_size, std::nullopt,
                                                      8, rx_size);
    EXPECT_TRUE(err_out.is_ok()) << err_out.error_value();
  });

  UnconfigureDevice();
}

TEST_F(VsockUsbTest, ShortFlagOnTxRequest) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  std::vector<uint8_t> short_data(500, 0xAB);
  ASSERT_TRUE(SocketWriteAll(&socket, short_data.data(), short_data.size()));
  ASSERT_TRUE(GetRxConcatExpect(short_data.data(), short_data.size()));

  std::vector<uint8_t> mtu_data(1024, 0xCD);
  ASSERT_TRUE(SocketWriteAll(&socket, mtu_data.data(), mtu_data.size()));
  ASSERT_TRUE(GetRxConcatExpect(mtu_data.data(), mtu_data.size()));

  std::vector<uint8_t> another_short(1, 0xEF);
  ASSERT_TRUE(SocketWriteAll(&socket, another_short.data(), another_short.size()));
  ASSERT_TRUE(GetRxConcatExpect(another_short.data(), another_short.size()));

  UnconfigureDevice();
}

// Tests re-entrant handling when UnconfigureEndpoints() is called while the driver is already
// in the ShuttingDown state (e.g. from an incoming FIDL call during teardown).
TEST_F(VsockUsbTest, TeardownRace) {
  ConfigureDevice();

  auto shutdown_done = std::make_shared<std::atomic<bool>>(false);
  auto completer = [shutdown_done]() { shutdown_done->store(true); };

  driver_test().RunInDriverContext([&](VsockUsb& driver) {
    VsockUsbTestHelper::Shutdown(driver, std::move(completer));

    zx_status_t status = VsockUsbTestHelper::UnconfigureEndpoints(driver);
    EXPECT_EQ(status, ZX_OK);
  });

  driver_test().runtime().RunUntil([&]() { return shutdown_done->load(); });
}

// Tests that multiple concurrent or post-shutdown callbacks registered with Shutdown() are all
// invoked reliably without dropping completers.
TEST_F(VsockUsbTest, MultipleShutdownCallbacks) {
  ConfigureDevice();

  auto shutdown_done_1 = std::make_shared<std::atomic<bool>>(false);
  auto shutdown_done_2 = std::make_shared<std::atomic<bool>>(false);
  auto shutdown_done_post = std::make_shared<std::atomic<bool>>(false);

  driver_test().RunInDriverContext([&](VsockUsb& driver) {
    VsockUsbTestHelper::Shutdown(driver, [shutdown_done_1]() { shutdown_done_1->store(true); });
    VsockUsbTestHelper::Shutdown(driver, [shutdown_done_2]() { shutdown_done_2->store(true); });
  });

  driver_test().runtime().RunUntil(
      [&]() { return shutdown_done_1->load() && shutdown_done_2->load(); });

  driver_test().RunInDriverContext([&](VsockUsb& driver) {
    VsockUsbTestHelper::Shutdown(driver,
                                 [shutdown_done_post]() { shutdown_done_post->store(true); });
  });

  EXPECT_TRUE(shutdown_done_post->load());
}

TEST_F(VsockUsbTest, UnconfigureAlreadyUnconfigured) {
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    zx_status_t status = VsockUsbTestHelper::UnconfigureEndpoints(driver);
    EXPECT_EQ(status, ZX_OK);
  });
}

TEST_F(VsockUsbTest, DISABLED_UnconfigureEndpointsDisableBadStateError) {
  ConfigureDevice();

  driver_test().RunInEnvironmentTypeContext(
      [](VsockUsbEnvironment& env) { env.fake_usb_->set_disable_ep_status(ZX_ERR_BAD_STATE); });

  driver_test().RunInEnvironmentTypeContext(
      [](VsockUsbEnvironment& env) { env.fake_usb_->ExpectDisableEndpoint(kBulkInEndpoint); });
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    zx_status_t status = VsockUsbTestHelper::UnconfigureEndpoints(driver);
    EXPECT_EQ(status, ZX_ERR_BAD_STATE);
  });

  driver_test().RunInEnvironmentTypeContext(
      [](VsockUsbEnvironment& env) { env.fake_usb_->set_disable_ep_status(std::nullopt); });
}

TEST_F(VsockUsbTest, DISABLED_UnconfigureEndpointsDisableOtherError) {
  ConfigureDevice();

  driver_test().RunInEnvironmentTypeContext(
      [](VsockUsbEnvironment& env) { env.fake_usb_->set_disable_ep_status(ZX_ERR_INTERNAL); });

  driver_test().RunInEnvironmentTypeContext(
      [](VsockUsbEnvironment& env) { env.fake_usb_->ExpectDisableEndpoint(kBulkInEndpoint); });
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    zx_status_t status = VsockUsbTestHelper::UnconfigureEndpoints(driver);
    EXPECT_EQ(status, ZX_ERR_INTERNAL);
  });

  driver_test().RunInEnvironmentTypeContext(
      [](VsockUsbEnvironment& env) { env.fake_usb_->set_disable_ep_status(std::nullopt); });
}

TEST_F(VsockUsbTest, DISABLED_ReadErrorUnconfiguresEndpoints) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  auto request = WaitForRequestOn(kBulkOutEndpoint);
  ASSERT_TRUE(request.has_value());

  ExpectDisableEndpoints();
  ASSERT_TRUE(CompleteMockUsbOutRequest(std::move(*request), kBulkOutEndpoint, nullptr, 0,
                                        ZX_ERR_INTERNAL));

  driver_test().runtime().RunUntil([&]() {
    bool offline = false;
    driver_test().RunInDriverContext(
        [&](VsockUsb& driver) { offline = !VsockUsbTestHelper::Online(driver); });
    return offline;
  });
}

TEST_F(VsockUsbTest, DISABLED_ReadErrorDisconnectLoopRegression) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  auto request = WaitForRequestOn(kBulkOutEndpoint);
  ASSERT_TRUE(request.has_value());

  ASSERT_TRUE(CompleteMockUsbOutRequest(std::move(*request), kBulkOutEndpoint, nullptr, 0,
                                        ZX_ERR_IO_NOT_PRESENT));

  driver_test().runtime().RunUntil([&]() {
    bool offline = false;
    driver_test().RunInDriverContext(
        [&](VsockUsb& driver) { offline = !VsockUsbTestHelper::Online(driver); });
    return offline;
  });
}

TEST_F(VsockUsbTest, CleanShutdownWithCanceledRequestsInFlight) {
  ConfigureDevice();
  // While running, RX requests are queued in flight on the out endpoint.
  driver_test().RunInDriverContext(
      [](VsockUsb& driver) { EXPECT_TRUE(driver.HasPendingRxRequests()); });

  // Connect socket and queue a TX write request so both RX and TX requests are in flight.
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  std::string_view test_data = "shutdown_tx_data";
  ASSERT_TRUE(SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(test_data.data()),
                             test_data.size()));
  driver_test().runtime().RunUntil([&]() {
    bool has_tx = false;
    driver_test().RunInDriverContext(
        [&](VsockUsb& driver) { has_tx = driver.HasPendingTxRequests(); });
    return has_tx;
  });

  // Stop the driver with both RX and TX requests in flight. StopDriver() invokes VsockUsb::Stop ->
  // Shutdown(), which cancels all in-flight requests and waits for them to drain back into the
  // request pool.
  zx::result<> result = driver_test().StopDriver();
  EXPECT_TRUE(result.is_ok());
  driver_stopped_ = true;
}

TEST_F(VsockUsbTest, SetInterfaceCompliance) {
  // 1. SetInterface called while device is unconfigured returns ZX_ERR_BAD_STATE.
  fidl::Result unconfigured_res = function_client_->SetInterface({{
      .interface = kInterfaceNum,
      .alt_setting = 0,
  }});
  ASSERT_TRUE(unconfigured_res.is_error());
  ASSERT_TRUE(unconfigured_res.error_value().is_domain_error());
  EXPECT_EQ(unconfigured_res.error_value().domain_error(), ZX_ERR_BAD_STATE);

  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  // 2. Bidirectional data transfer over active socket before SetInterface.
  std::string_view test_target_data = "Data from target before SetInterface";
  ASSERT_TRUE(SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(test_target_data.data()),
                             test_target_data.size()));
  ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_target_data.data()),
                                test_target_data.size()));

  std::string_view test_host_data = "Data from host before SetInterface";
  ASSERT_TRUE(
      SendTx(reinterpret_cast<const uint8_t*>(test_host_data.data()), test_host_data.size()));
  ASSERT_TRUE(SocketReadExpect(&socket, reinterpret_cast<const uint8_t*>(test_host_data.data()),
                               test_host_data.size()));

  // 3. Standard host enumeration SET_INTERFACE(kInterfaceNum, 0).
  // Must return ZX_OK and NOT reconfigure endpoints or destroy the active socket.
  fidl::Result result = function_client_->SetInterface({{
      .interface = kInterfaceNum,
      .alt_setting = 0,
  }});
  ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
  driver_test().runtime().RunUntilIdle();

  // Socket must remain active and not closed.
  zx_signals_t pending = 0;
  zx_status_t wait_status =
      socket.wait_one(ZX_SOCKET_PEER_CLOSED, zx::time::infinite_past(), &pending);
  EXPECT_EQ(wait_status, ZX_ERR_TIMED_OUT);

  // In-flight RX requests remain queued and pending.
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    EXPECT_TRUE(driver.HasPendingRxRequests());
    EXPECT_FALSE(driver.HasPendingTxRequests());
  });

  // 4. Bidirectional data transfer continues seamlessly on the existing socket.
  std::string_view test_target_data_b = "Data from target after SetInterface";
  ASSERT_TRUE(SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(test_target_data_b.data()),
                             test_target_data_b.size()));
  ASSERT_TRUE(GetRxConcatExpect(reinterpret_cast<const uint8_t*>(test_target_data_b.data()),
                                test_target_data_b.size()));

  std::string_view test_host_data_b = "Data from host after SetInterface";
  ASSERT_TRUE(
      SendTx(reinterpret_cast<const uint8_t*>(test_host_data_b.data()), test_host_data_b.size()));
  ASSERT_TRUE(SocketReadExpect(&socket, reinterpret_cast<const uint8_t*>(test_host_data_b.data()),
                               test_host_data_b.size()));

  // 5. Unexpected interface returns ZX_ERR_NOT_SUPPORTED.
  fidl::Result bad_interface = function_client_->SetInterface({{
      .interface = static_cast<uint8_t>(kInterfaceNum + 1),
      .alt_setting = 0,
  }});
  ASSERT_TRUE(bad_interface.is_error());
  ASSERT_TRUE(bad_interface.error_value().is_domain_error());
  EXPECT_EQ(bad_interface.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);

  // 6. Unexpected alternate setting returns ZX_ERR_NOT_SUPPORTED.
  fidl::Result bad_alt = function_client_->SetInterface({{
      .interface = kInterfaceNum,
      .alt_setting = 1,
  }});
  ASSERT_TRUE(bad_alt.is_error());
  ASSERT_TRUE(bad_alt.error_value().is_domain_error());
  EXPECT_EQ(bad_alt.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);

  UnconfigureDevice();
}

TEST_F(VsockUsbTest, UnconfigureEndpointsCancelsInFlightRequests) {
  ConfigureDevice();
  auto callback = SetupCallback(1);
  zx::socket socket = WaitForSocket(*callback);
  ASSERT_TRUE(socket.is_valid());

  // Queue a TX request into bulk IN so HasPendingTxRequests() becomes true.
  std::string_view tx_data = "Pending TX in-flight payload";
  ASSERT_TRUE(
      SocketWriteAll(&socket, reinterpret_cast<const uint8_t*>(tx_data.data()), tx_data.size()));
  driver_test().runtime().RunUntil([&]() {
    bool has_tx = false;
    driver_test().RunInDriverContext(
        [&](VsockUsb& driver) { has_tx = driver.HasPendingTxRequests(); });
    return has_tx;
  });

  // Both RX and TX requests are now in flight.
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    EXPECT_TRUE(driver.HasPendingRxRequests());
    EXPECT_TRUE(driver.HasPendingTxRequests());
  });

  // UnconfigureDevice invokes UnconfigureEndpoints(), which calls CancelAllEndpoints()
  // on both endpoints and flushes/reclaims all in-flight RX and TX requests back to the pool.
  UnconfigureDevice();
  driver_test().runtime().RunUntil([&]() {
    bool has_pending = true;
    driver_test().RunInDriverContext([&](VsockUsb& driver) {
      has_pending = driver.HasPendingRxRequests() || driver.HasPendingTxRequests();
    });
    return !has_pending;
  });

  // Verify all requests are back in the pool and no requests remain in flight.
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    EXPECT_FALSE(driver.HasPendingRxRequests());
    EXPECT_FALSE(driver.HasPendingTxRequests());
  });

  // Reconfigure immediately to confirm all requests can be re-queued without starvation.
  ConfigureDevice();
  driver_test().RunInDriverContext([](VsockUsb& driver) {
    EXPECT_TRUE(driver.HasPendingRxRequests());
    EXPECT_FALSE(driver.HasPendingTxRequests());
  });

  UnconfigureDevice();
}

TEST_F(VsockUsbTest, ReconfigurationDeliversSocketWithoutResettingCallback) {
  ConfigureDevice();
  // Expect two socket deliveries on the same callback across reconfiguration.
  auto callback = SetupCallback(2);
  zx::socket socket0 = WaitForSocket(*callback);
  ASSERT_TRUE(socket0.is_valid());

  // Simulate USB disconnect / bus reset (e.g. adbd restarting on adb root/unroot).
  UnconfigureDevice();
  driver_test().runtime().RunUntilIdle();

  // Re-configure without re-registering SetCallback.
  ConfigureDevice();
  zx::socket socket1 = WaitForSocket(*callback);
  ASSERT_TRUE(socket1.is_valid());

  // Verify that the old socket was torn down and peer was closed.
  zx_signals_t pending;
  ASSERT_EQ(socket0.wait_one(ZX_SOCKET_PEER_CLOSED, zx::time::infinite(), &pending), ZX_OK);
  ASSERT_NE(pending & ZX_SOCKET_PEER_CLOSED, 0u);

  // Verify that data can be transmitted across the reconfigured socket.
  std::string_view test_data = "reconfigured_socket_data";
  ASSERT_TRUE(SendTx(reinterpret_cast<const uint8_t*>(test_data.data()), test_data.size()));
  ASSERT_TRUE(SocketReadExpect(&socket1, reinterpret_cast<const uint8_t*>(test_data.data()),
                               test_data.size()));

  UnconfigureDevice();
}
// NOLINTEND(readability-container-data-pointer)
// NOLINTEND(readability-convert-member-functions-to-static)
// NOLINTEND(misc-use-anonymous-namespace)
