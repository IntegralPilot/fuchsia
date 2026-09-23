// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_ENDPOINT_INCLUDE_USB_ENDPOINT_USB_ENDPOINT_CLIENT_H_
#define SRC_DEVICES_USB_LIB_USB_ENDPOINT_INCLUDE_USB_ENDPOINT_USB_ENDPOINT_CLIENT_H_

#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>

#ifdef DFV2_COMPAT_LOGGING
#include <lib/driver/compat/cpp/logging.h>  // nogncheck
#else
#include <lib/ddk/debug.h>  // nogncheck
#endif

#include <mutex>

#include <usb/request-fidl.h>

namespace usb {

namespace internal {

// EndpointClientBase is self contained helps manage common functionality for the client side of
// `fuchsia_hardware_usb_endpoint::Endpoint` without needing any references outside.
// EndpointClientBase should only be inherited by EndpointClient and should not be used
// independently. One of the largest uses of EndpointClientBase is managing mapped VMOs.
class EndpointClientBase {
 public:
  explicit EndpointClientBase(usb::EndpointType ep_type) : ep_type_(ep_type) {}
  // Upon destruction, EndpointClientBase ensures that all allocated requests have been freed and
  // unmaps VMOs.
  ~EndpointClientBase();

  // Only allow access to client_. Generally this should only be used to call GetInfo,
  // QueueRequests, and CancelAll, where RegisterVmos and UnregisterVmos will be called accordingly
  // by AddRequests and DeleteRequest.
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint>& operator->() { return client_; }
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint>& client() { return client_; }

  // Helper functions that manage access to the request pool. Buffer regions of a request will be
  // mapped upon addition to the pool. If mapping upon addition is not desired, one may use
  // fuchsia_hardware_usb_request::Buffer::Tag::kData types or manage its own requests (i.e. not
  // using a pool). Note that all functions specified in EndpointClient expect that the requests
  // have been previously mapped and the mapped addresses are saved and managed by EndpointClient.
  size_t AddRequests(size_t req_count, size_t size, fuchsia_hardware_usb_request::Buffer::Tag type);
  std::optional<usb::FidlRequest> GetRequest() { return free_reqs_.Get(); }
  void PutRequest(usb::FidlRequest&& request) { free_reqs_.Put(std::move(request)); }
  bool RequestsFull() { return free_reqs_.Full(); }
  bool RequestsEmpty() { return free_reqs_.Empty(); }
  size_t GetInFlightCount() { return free_reqs_.GetInFlightCount(); }
  size_t GetTotalCount() { return free_reqs_.GetTotalCount(); }
  // Helper function that deletes a request from the pool. If this function is not called when
  // deleting a request from the pool, it will stay mapped (and registered) until the endpoint is
  // destructed.
  zx_status_t DeleteRequest(usb::FidlRequest&& request) __TA_REQUIRES(mutex_);

  // Closes the endpoint client. This will free all requests and unmap all VMOs.
  zx_status_t Close() __TA_EXCLUDES(mutex_);

  std::mutex& mutex() __TA_RETURN_CAPABILITY(mutex_) { return mutex_; }

  constexpr auto GetMapped() {
    return [this](const fuchsia_hardware_usb_request::Buffer& buffer) {
      std::lock_guard<std::mutex> _(mutex_);
      return get_mapped(buffer);
    };
  }

  constexpr auto GetMappedLocked() {
    return [this](const fuchsia_hardware_usb_request::Buffer& buffer)
               __TA_REQUIRES(mutex_) { return get_mapped(buffer); };
  }

  std::optional<zx_vaddr_t> GetMappedAddr(const fuchsia_hardware_usb_request::Request& request,
                                          size_t idx) {
    std::lock_guard<std::mutex> _(mutex_);
    return GetMappedAddrLocked(request, idx);
  }
  std::optional<zx_vaddr_t> GetMappedAddrLocked(
      const fuchsia_hardware_usb_request::Request& request, size_t idx) __TA_REQUIRES(mutex_) {
    if (!request.data().has_value() || idx >= request.data()->size()) {
      return std::nullopt;
    }
    auto mapped = get_mapped(*request.data()->at(idx).buffer());
    return mapped.is_ok() ? std::make_optional<zx_vaddr_t>(mapped->addr) : std::nullopt;
  }

 protected:
  // client_: protected so EndpointClient can access it in Init()
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint> client_;

 private:
  // Registers vmo_count VMOs with size vmo_size. Maps these VMOs and inserts corresponding requests
  // into the free_reqs_ pool. Returns the number of VMOs successfully registered. Called by
  // AddRequests.
  size_t RegisterVmos(size_t vmo_count, size_t vmo_size);

  std::mutex mutex_;

  // Unmaps a buffer region.
  zx_status_t Unmap(const fuchsia_hardware_usb_request::BufferRegion& buffer) __TA_REQUIRES(mutex_);

  // Gets mapped address
  zx::result<std::optional<usb::MappedVmo>> get_mapped(
      const fuchsia_hardware_usb_request::Buffer& buffer) __TA_REQUIRES(mutex_);

  const usb::EndpointType ep_type_;

  // free_reqs_: Free request pool with buffer field filled out for VMO and VMO_IDs. Other fields
  // should be taken as uninitialized and may contain remnants of its previous lifetime.
  usb::FidlRequestPool free_reqs_;

  // vmo_mapped_addrs_: maps buffer_id or VMO handle to mapped virtual address.
  std::map<uint64_t, usb::MappedVmo> vmo_mapped_addrs_ __TA_GUARDED(mutex_);

  // Internal buffer_id counter used to produce unique buffer_ids for `RegisterVmos`.
  std::atomic_uint32_t buffer_id_ = 0;
};

}  // namespace internal

// EndpointClient helps manage common functionality for the client side of
// `fuchsia_hardware_usb_endpoint::Endpoint`. Most notably, EndpointClient binds a client to make
// calls such as `QueueRequest` and `RegisterVmos` and implements the corresponding
// `fidl::AsyncEventHandler<fuchsia_hardware_usb_endpoint::Endpoint>` required to handle
// `OnCompletion` events. EndpointClient is templated on `DeviceType` which should have a `void
// (fuchsia_hardware_usb_endpoint::Completion)` function, which will be called for each completion
// event received. All other common functionality implemented by EndpointClient are described in
// detail in the `EndpointClientBase` class, which EndpointClient inherits from.
//
// Example Usage:
//   class SampleDeviceType {
//    public:
//    private:
//     void RequestComplete(fuchsia_hardware_usb_endpoint::Completion completion);
//
//     usb_endpoint::EndpointClient<SampleDeviceType> ep_{usb_endpoint::EndpointType::BULK, this,
//                                                     std::mem_fn(&SampleDeviceType::RequestComplete)};
//   };
template <class DeviceType>
class EndpointClient : public internal::EndpointClientBase,
                       public fidl::AsyncEventHandler<fuchsia_hardware_usb_endpoint::Endpoint> {
 public:
  using OnCompletionFuncType =
      std::__mem_fn<void (DeviceType::*)(std::vector<fuchsia_hardware_usb_endpoint::Completion>)>;

  EndpointClient(usb::EndpointType ep_type, DeviceType* device, OnCompletionFuncType on_completion)
      : internal::EndpointClientBase(ep_type), device_(device), on_completion_(on_completion) {}

  // Init is templated on `ProtocolType`, which declares `ConnectToEndpoint(uint8_t ep_addr,
  // fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint>)`--either `fuchsia_hardware_usb::Usb`
  // or `fuchsia_hardware_usb_function::UsbFunction`. Init creates a connection between the server
  // side endpoint and binds the client side to this.
  template <typename ProtocolType>
  zx_status_t Init(uint8_t ep_addr, fidl::ClientEnd<ProtocolType>& client,
                   async_dispatcher_t* dispatcher);

  zx_status_t Init(fidl::ClientEnd<fuchsia_hardware_usb_endpoint::Endpoint> client_end,
                   async_dispatcher_t* dispatcher) {
    client_.Bind(std::move(client_end), dispatcher, this);
    return ZX_OK;
  }

  // fidl::AsyncEventHandler implementation.
  // OnCompletion: handles completed requests by calling on_completion_ for each request completed.
  void OnCompletion(
      fidl::Event<fuchsia_hardware_usb_endpoint::Endpoint::OnCompletion>& event) override {
    on_completion_(device_, std::move(event.completion()));
  }

  void on_fidl_error(fidl::UnbindInfo error) override {
    if (error.status() == ZX_ERR_CANCELED) {
      zxlogf(DEBUG, "on_fidl_error: %s", error.FormatDescription().c_str());
    } else {
      zxlogf(ERROR, "on_fidl_error: %s", error.FormatDescription().c_str());
    }
    if (on_unbind_) {
      auto cb = std::move(on_unbind_);
      cb();
    }
  }

  // Sets a callback to be executed once when the endpoint's FIDL channel unbinds
  // (e.g. peer closed, cancelled). The callback is one-shot.
  void SetOnUnbind(fit::closure callback) { on_unbind_ = std::move(callback); }

 private:
  // device_: pointer to device implementing on_completion_. Should not and will not outlive
  // EndpointClient if EndpointClient is declared as a member of device_ as in the example above.
  DeviceType* device_;
  // on_completion_: member function of device_ that is called for each request completed.
  OnCompletionFuncType on_completion_;
  fit::closure on_unbind_;
};

template <class DeviceType>
template <typename ProtocolType>
zx_status_t EndpointClient<DeviceType>::Init(uint8_t ep_addr, fidl::ClientEnd<ProtocolType>& client,
                                             async_dispatcher_t* dispatcher) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (endpoints.is_error()) {
    zxlogf(ERROR, "Creating endpoint error: %s", zx_status_get_string(endpoints.status_value()));
    return endpoints.status_value();
  }
  auto result = fidl::Call(client)->ConnectToEndpoint({ep_addr, std::move(endpoints->server)});
  if (result.is_error()) {
    zxlogf(ERROR, "ConnectToEndpoint failed =: %s",
           result.error_value().FormatDescription().c_str());
    return result.error_value().is_framework_error()
               ? result.error_value().framework_error().status()
               : ZX_ERR_INTERNAL;
  }
  client_.Bind(std::move(endpoints->client), dispatcher, this);
  if (!client_.is_valid()) {
    zxlogf(ERROR, "Could not bind to endpoint!");
    return ZX_ERR_CONNECTION_REFUSED;
  }

  return ZX_OK;
}

}  // namespace usb

#endif  // SRC_DEVICES_USB_LIB_USB_ENDPOINT_INCLUDE_USB_ENDPOINT_USB_ENDPOINT_CLIENT_H_
