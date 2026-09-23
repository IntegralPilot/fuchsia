// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_ENDPOINT_TESTING_FAKE_USB_ENDPOINT_SERVER_H_
#define SRC_DEVICES_USB_LIB_USB_ENDPOINT_TESTING_FAKE_USB_ENDPOINT_SERVER_H_

#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <mutex>
#include <queue>
#include <variant>

#ifdef USE_ZXTEST
#include <zxtest/zxtest.h>
#else
#include <gtest/gtest.h>
#endif

namespace fake_usb_endpoint {

// FakeEndpoint generally should not be used unless accessed from FakeUsbFidlProvider, but may be
// overridden for specific use-cases.
class FakeEndpoint : public fidl::Server<fuchsia_hardware_usb_endpoint::Endpoint> {
 public:
  ~FakeEndpoint() __TA_NO_THREAD_SAFETY_ANALYSIS {
    EXPECT_TRUE(expected_get_info_.empty());
    EXPECT_TRUE(requests_.empty());
    EXPECT_TRUE(completions_.empty());
  }

  virtual void Connect(async_dispatcher_t* dispatcher,
                       fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server) {
    std::lock_guard<std::mutex> _(lock_);
    binding_ref_.emplace(fidl::BindServer(dispatcher, std::move(server), this));
  }

  // RequestComplete: responds to the next request. If there are any requests in the request queue,
  // respond to that. If not, save this response and respond with the next incoming request.
  void RequestComplete(zx_status_t status, size_t actual) {
    RequestCompleteAny(status, QueuedRequestComplete(actual));
  }

  void RequestComplete(zx_status_t status, std::vector<uint8_t> data) {
    RequestCompleteAny(status, QueuedRequestComplete(std::move(data)));
  }

  // GetInfo: responds according to previous calls of ExpectGetInfo() and returns
  //  * error status: if previous call of ExpectedGetInfo() indicated that the status to return is
  //                  not ZX_OK
  //  * info: if previous call of ExpectedGetInfo() indicated that the status to return is ZX_OK,
  //          returns the info from ExpectedGetInfo()
  void GetInfo(GetInfoCompleter::Sync& completer) override {
    std::lock_guard<std::mutex> _(lock_);
    EXPECT_FALSE(expected_get_info_.empty());
    if (expected_get_info_.front().first != ZX_OK) {
      completer.Reply(fit::as_error(expected_get_info_.front().first));
      expected_get_info_.pop();
      return;
    }

    completer.Reply(fit::ok(std::move(expected_get_info_.front().second)));
    expected_get_info_.pop();
  }
  // QueueRequests: adds requests to a queue, which will be replied to when RequestComplete() is
  // called or if there is already a completion saved from before.
  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override {
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    {
      std::lock_guard<std::mutex> _(lock_);
      local_binding = binding_ref_;

      if (!enabled_) {
        for (auto& req : request.req()) {
          completions.emplace_back(std::move(fuchsia_hardware_usb_endpoint::Completion()
                                                 .request(std::move(req))
                                                 .status(ZX_ERR_BAD_STATE)
                                                 .transfer_size(0)));
        }
      } else {
        // Add request to queue.
        requests_.insert(requests_.end(), std::make_move_iterator(request.req().begin()),
                         std::make_move_iterator(request.req().end()));

        // Reply if there is a completion saved for it already.
        while (!completions_.empty() && !requests_.empty()) {
          zx_status_t status = completions_.front().first;
          auto data = std::move(completions_.front().second);
          completions_.pop();
          auto completion = RequestCompleteLocked(status, std::move(data));
          if (!completion.has_value()) {
            break;
          }
          completions.emplace_back(std::move(completion.value()));
        }
      }
    }
    if (!completions.empty() && local_binding.has_value()) {
      // Ignore failures; the client may have already closed the channel.
      auto _ = fidl::SendEvent(*local_binding)->OnCompletion(std::move(completions));
    }
  }

  void CancelAll(CancelAllCompleter::Sync& completer) override {
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    {
      std::lock_guard<std::mutex> _(lock_);
      local_binding = binding_ref_;
      completions = CancelAllLocked(/*is_disable=*/false);
    }
    if (!completions.empty() && local_binding.has_value()) {
      // Ignore failures; the client may have already closed the channel.
      auto _ = fidl::SendEvent(*local_binding)->OnCompletion(std::move(completions));
    }
    completer.Reply(fit::ok());
  }

  void SetNextCancelStatus(zx_status_t status, size_t transfer_size = 0) {
    std::lock_guard<std::mutex> _(lock_);
    next_cancel_status_ = status;
    next_cancel_transfer_size_ = transfer_size;
  }

  void SetEnabled(bool enabled) {
    std::lock_guard<std::mutex> _(lock_);
    enabled_ = enabled;
  }

  void DisableAndCancelAll() {
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    {
      std::lock_guard<std::mutex> _(lock_);
      local_binding = binding_ref_;
      completions = CancelAllLocked(/*is_disable=*/true);
    }
    if (!completions.empty() && local_binding.has_value()) {
      // Ignore failures; the client may have already closed the channel.
      auto _ = fidl::SendEvent(*local_binding)->OnCompletion(std::move(completions));
    }
  }

  // RegisterVmos: creates VMOs on demand.
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) override {
    std::lock_guard<std::mutex> _(lock_);
    std::vector<fuchsia_hardware_usb_endpoint::VmoHandle> ret;
    for (const auto& vmo_id : request.vmo_ids()) {
      if (vmos_.contains(vmo_id.id().value())) {
        continue;
      }
      zx::vmo vmo;
      zx_status_t status = zx::vmo::create(*vmo_id.size(), 0, &vmo);
      if (status != ZX_OK) {
        continue;
      }
      zx::vmo duplicate;
      status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
      if (status != ZX_OK) {
        continue;
      }
      vmos_.emplace(*vmo_id.id(), std::move(vmo));
      ret.emplace_back(std::move(
          fuchsia_hardware_usb_endpoint::VmoHandle().id(vmo_id.id()).vmo(std::move(duplicate))));
    }
    completer.Reply(std::move(ret));
  }
  // UnregisterVmos: succeeds without checking anything.
  void UnregisterVmos(UnregisterVmosRequest& request,
                      UnregisterVmosCompleter::Sync& completer) override {
    std::lock_guard<std::mutex> _(lock_);
    fuchsia_hardware_usb_endpoint::EndpointUnregisterVmosResponse response;
    std::vector<zx_status_t>& errors = response.errors();
    std::vector<fuchsia_hardware_usb_request::VmoId>& failed_vmo_ids = response.failed_vmo_ids();
    for (const auto& vmo_id : request.vmo_ids()) {
      size_t erased = vmos_.erase(vmo_id);
      if (erased == 0) {
        errors.emplace_back(ZX_ERR_NOT_FOUND);
        failed_vmo_ids.emplace_back(vmo_id);
      }
    }
    completer.Reply(std::move(response));
  }

  // ExpectGetInfo
  //  * status: status to return on GetInfo()
  //  * info: if status is ZX_OK, return this info.
  virtual void ExpectGetInfo(zx_status_t status, fuchsia_hardware_usb_endpoint::EndpointInfo info) {
    std::lock_guard<std::mutex> _(lock_);
    expected_get_info_.emplace(status, std::move(info));
  }

  size_t pending_request_count() {
    std::lock_guard<std::mutex> _(lock_);
    return requests_.size();
  }

  // Duplicates and returns a previously registered VMO with vmo_id.
  zx::result<zx::vmo> GetVmo(fuchsia_hardware_usb_request::VmoId vmo_id) {
    std::lock_guard<std::mutex> _(lock_);
    auto it = vmos_.find(vmo_id);
    if (it == vmos_.end()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    zx::vmo duplicate;
    zx_status_t status = it->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(duplicate));
  }

  // Reads the value of the pending request at the front of the queue.
  zx::result<std::vector<uint8_t>> ReadPendingRequestData() {
    std::lock_guard<std::mutex> _(lock_);
    if (requests_.empty()) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
    std::vector<uint8_t> ret;
    const fuchsia_hardware_usb_request::Request& request = requests_.front();
    if (!request.data().has_value()) {
      return zx::error(ZX_ERR_IO_INVALID);
    }
    for (auto& region : request.data().value()) {
      if (!region.buffer().has_value()) {
        return zx::error(ZX_ERR_IO_INVALID);
      }
      auto& buffer = region.buffer().value();
      switch (buffer.Which()) {
        case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId: {
          if (!region.offset().has_value() || !region.size().has_value()) {
            return zx::error(ZX_ERR_IO_INVALID);
          }
          auto it = vmos_.find(buffer.vmo_id().value());
          if (it == vmos_.end()) {
            return zx::error(ZX_ERR_NOT_FOUND);
          }
          zx::vmo& vmo = it->second;
          ret.resize(ret.size() + region.size().value());
          zx_status_t status = vmo.read(ret.data() + ret.size() - region.size().value(),
                                        region.offset().value(), region.size().value());
          if (status != ZX_OK) {
            return zx::error(status);
          }
        } break;
        case fuchsia_hardware_usb_request::Buffer::Tag::kData: {
          const std::vector<uint8_t>& data = buffer.data().value();
          ret.insert(ret.end(), data.begin(), data.end());
        } break;
        default:
          return zx::error(ZX_ERR_NOT_SUPPORTED);
      }
    }
    return zx::ok(std::move(ret));
  }

 protected:
  using QueuedRequestComplete = std::variant<size_t, std::vector<uint8_t>>;

  void RequestCompleteAny(zx_status_t status, QueuedRequestComplete actual_or_data) {
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> local_binding;
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    {
      std::lock_guard<std::mutex> _(lock_);
      // We must not early exit if `binding_ref_` is empty here. Tests may synthesize
      // hardware state (like an unplug event) and queue completions before the
      // endpoint connection is fully established. `RequestCompleteLocked` will safely
      // stash these completions in `completions_` so they are delivered when requests arrive.
      auto completion = RequestCompleteLocked(status, std::move(actual_or_data));
      if (completion.has_value()) {
        local_binding = binding_ref_;
        completions.emplace_back(std::move(completion.value()));
      }
    }
    if (!completions.empty() && local_binding.has_value()) {
      // Ignore failures; the client may have already closed the channel.
      auto _ = fidl::SendEvent(*local_binding)->OnCompletion(std::move(completions));
    }
  }

  std::optional<fuchsia_hardware_usb_endpoint::Completion> RequestCompleteLocked(
      zx_status_t status, QueuedRequestComplete actual_or_data) __TA_REQUIRES(lock_) {
    if (!enabled_) {
      return std::nullopt;
    }
    if (requests_.empty()) {
      // Save completion for next incoming request.
      completions_.emplace(status, std::move(actual_or_data));
      return std::nullopt;
    }

    fuchsia_hardware_usb_request::Request& request = requests_.front();
    size_t transfer_size = 0;

    if (std::holds_alternative<size_t>(actual_or_data)) {
      transfer_size = std::get<size_t>(actual_or_data);
    } else {
      std::vector<uint8_t>& vec = std::get<std::vector<uint8_t>>(actual_or_data);
      transfer_size = vec.size();

      size_t written = 0;
      for (auto& region : request.data().value()) {
        auto& buffer = region.buffer().value();
        switch (buffer.Which()) {
          case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId: {
            if (written >= transfer_size) {
              region.size(0);
            } else {
              auto it = vmos_.find(buffer.vmo_id().value());
              if (it != vmos_.end()) {
                zx::vmo& vmo = it->second;
                size_t chunk = std::min(region.size().value(), transfer_size - written);
                vmo.write(vec.data() + written, region.offset().value(), chunk);
                written += chunk;
                region.size(chunk);
              }
            }
          } break;
          case fuchsia_hardware_usb_request::Buffer::Tag::kData: {
            if (written >= transfer_size) {
              region.buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>{}));
              region.size(0);
            } else {
              size_t chunk =
                  std::min(static_cast<size_t>(fuchsia_hardware_usb_request::kMaxTransferSize),
                           transfer_size - written);
              auto start_it = vec.begin() + static_cast<std::ptrdiff_t>(written);
              auto end_it = vec.begin() + static_cast<std::ptrdiff_t>(written + chunk);
              std::vector<uint8_t> chunk_vec(start_it, end_it);
              region.buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::move(chunk_vec)));
              region.size(chunk);
              written += chunk;
            }
          } break;
          default:
            ZX_PANIC("unsupported buffer type %d", buffer.Which());
            break;
        }
      }
    }

    // Respond to the next request in the queue.
    auto completion = std::move(fuchsia_hardware_usb_endpoint::Completion()
                                    .request(std::move(request))
                                    .status(status)
                                    .transfer_size(transfer_size));
    requests_.erase(requests_.begin());
    return std::move(completion);
  }

  std::vector<fuchsia_hardware_usb_endpoint::Completion> CancelAllLocked(bool is_disable)
      __TA_REQUIRES(lock_) {
    if (is_disable) {
      enabled_ = false;
      completions_ = {};
    }
    std::vector<fuchsia_hardware_usb_request::Request> requests = std::move(requests_);
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;
    completions.reserve(requests.size());
    for (auto& request : requests) {
      zx_status_t status = ZX_ERR_CANCELED;
      size_t transfer_size = 0;
      if (next_cancel_status_.has_value()) {
        status = *next_cancel_status_;
        transfer_size = next_cancel_transfer_size_;
        next_cancel_status_.reset();
      }
      completions.emplace_back(std::move(fuchsia_hardware_usb_endpoint::Completion()
                                             .request(std::move(request))
                                             .status(status)
                                             .transfer_size(transfer_size)));
    }
    return completions;
  }

  std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> binding_ref_
      __TA_GUARDED(lock_);

  std::mutex lock_;
  std::queue<std::pair<zx_status_t, fuchsia_hardware_usb_endpoint::EndpointInfo>> expected_get_info_
      __TA_GUARDED(lock_);
  std::vector<fuchsia_hardware_usb_request::Request> requests_ __TA_GUARDED(lock_);
  std::queue<std::pair<zx_status_t, QueuedRequestComplete>> completions_ __TA_GUARDED(lock_);
  std::unordered_map<uint64_t, zx::vmo> vmos_ __TA_GUARDED(lock_);
  std::optional<zx_status_t> next_cancel_status_ __TA_GUARDED(lock_);
  size_t next_cancel_transfer_size_ __TA_GUARDED(lock_);
  bool enabled_ __TA_GUARDED(lock_) = true;
};

template <typename ProtocolType, typename FakeEndpointType>
class FakeUsbFidlProviderBase : public fidl::Server<ProtocolType> {
 public:
  explicit FakeUsbFidlProviderBase(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}
  virtual ~FakeUsbFidlProviderBase() __TA_NO_THREAD_SAFETY_ANALYSIS {
    EXPECT_TRUE(expected_connect_to_endpoint_.empty());
  }

  virtual void ExpectConnectToEndpoint(uint8_t ep_addr) {
    std::lock_guard<std::mutex> _(lock_);
    expected_connect_to_endpoint_.push(ep_addr);
  }

  FakeEndpointType& fake_endpoint(uint8_t ep_addr) {
    std::lock_guard<std::mutex> _(lock_);
    // Returning a reference while locked is safe because `std::map` guarantees reference stability
    // and FakeEndpoint manages its own internal state lock.
    return fake_endpoints_[ep_addr];
  }

  void ConnectToEndpoint(
      fidl::Request<typename ProtocolType::ConnectToEndpoint>& request,
      typename fidl::internal::NaturalCompleter<typename ProtocolType::ConnectToEndpoint>::Sync&
          completer) override {
    {
      std::lock_guard<std::mutex> _(lock_);
      EXPECT_FALSE(expected_connect_to_endpoint_.empty());

      uint8_t expected = expected_connect_to_endpoint_.front();
      expected_connect_to_endpoint_.pop();
      EXPECT_EQ(expected, request.ep_addr());
      // Calling Connect() (which acquires the Endpoint lock) while holding the Provider lock
      // establishes a safe Provider -> Endpoint lock hierarchy and avoids lock acquire/release
      // churn.
      fake_endpoints_[expected].Connect(dispatcher_, std::move(request.ep()));
    }
    completer.Reply(fit::ok());
  }

 protected:
  async_dispatcher_t* dispatcher() const { return dispatcher_; }

 private:
  std::mutex lock_;
  async_dispatcher_t* dispatcher_;

  std::queue<uint8_t> expected_connect_to_endpoint_ __TA_GUARDED(lock_);

  std::map<uint8_t, FakeEndpointType> fake_endpoints_ __TA_GUARDED(lock_);
};

// FakeUsbFidlProvider is, as its name suggests, a fake USB FIDL server for testing.
//
// ProtocolType must be one of fuchsia_hardware_usb_dci::UsbDci,
// fuchsia_usb_hardware_function::UsbFunction, or fuchsia_hardware_usb::Usb. In other words,
// ProtocolType is expected to have one function to override--void
// ConnectToEndpoint(ConnectToEndpointRequest& request, ConnectToEndpointCompleter::Sync&
// completer).
//
// fuchsia_hardware_usb_hci::UsbHci may also use this fake USB FIDL server, but will
// have to override the ConnectToEndpoint and write a new ExpectConnectToEndpoint method to
// accommodate device_id.
//
// A specialization is provided for fuchsia_hardware_usb_function::UsbFunction
// that stubs all calls.
//
// It provides connections to several FakeEndpoints as requested. FakeEndpointType must be
// FakeEndpoint or an inherited class of FakeEndpoint, defaulting to FakeEndpoint if not
// specified.
template <typename ProtocolType, typename FakeEndpointType = FakeEndpoint>
class FakeUsbFidlProvider : public FakeUsbFidlProviderBase<ProtocolType, FakeEndpointType> {
 public:
  using FakeUsbFidlProviderBase<ProtocolType, FakeEndpointType>::FakeUsbFidlProviderBase;
};

template <typename FakeEndpointType>
class FakeUsbFidlProvider<fuchsia_hardware_usb_function::UsbFunction, FakeEndpointType>
    : public FakeUsbFidlProviderBase<fuchsia_hardware_usb_function::UsbFunction, FakeEndpointType> {
 public:
  using Base =
      FakeUsbFidlProviderBase<fuchsia_hardware_usb_function::UsbFunction, FakeEndpointType>;
  using Base::Base;

  void Configure(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure>& request,
      fidl::internal::NaturalCompleter<fuchsia_hardware_usb_function::UsbFunction::Configure>::Sync&
          completer) override {
    completer.Reply(fit::ok());
  }

  void AllocResources(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::AllocResources>::Sync& completer) override {
    fuchsia_hardware_usb_function::UsbFunctionAllocResourcesResponse response;
    response.interface_nums() = {};
    response.endpoint_addrs() = {};
    response.string_indices() = {};
    completer.Reply(fit::ok(std::move(response)));
  }

  void EndpointSetStall(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::EndpointSetStall>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::EndpointSetStall>::Sync& completer) override {
    completer.Reply(fit::ok());
  }
  void EndpointClearStall(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::EndpointClearStall>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::EndpointClearStall>::Sync& completer)
      override {
    completer.Reply(fit::ok());
  }

  void ConfigureEndpoint(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>::Sync& completer)
      override {
    this->fake_endpoint(request.endpoint_address()).SetEnabled(true);
    completer.Reply(fit::ok());
  }

  void DisableEndpoint(
      fidl::Request<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>& request,
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>::Sync& completer) override {
    this->fake_endpoint(request.endpoint_address()).DisableAndCancelAll();
    completer.Reply(fit::ok());
  }

  void Deconfigure(
      fidl::internal::NaturalCompleter<
          fuchsia_hardware_usb_function::UsbFunction::Deconfigure>::Sync& completer) override {
    completer.Reply(fit::ok());
  }
};

}  // namespace fake_usb_endpoint

#endif  // SRC_DEVICES_USB_LIB_USB_ENDPOINT_TESTING_FAKE_USB_ENDPOINT_SERVER_H_
