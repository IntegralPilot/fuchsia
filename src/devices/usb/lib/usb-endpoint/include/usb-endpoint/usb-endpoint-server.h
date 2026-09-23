// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_ENDPOINT_INCLUDE_USB_ENDPOINT_USB_ENDPOINT_SERVER_H_
#define SRC_DEVICES_USB_LIB_USB_ENDPOINT_INCLUDE_USB_ENDPOINT_USB_ENDPOINT_SERVER_H_

#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <lib/fit/result.h>
#include <lib/io-buffer/phys-iter.h>
#include <lib/zx/eventpair.h>

#include <mutex>
#include <utility>

#include <usb/request-cpp.h>
#include <usb/request-fidl.h>

namespace usb {

using RequestVariant = std::variant<usb::BorrowedRequest<void>, usb::FidlRequest>;

enum class ScatterGatherSupport {
  kSupported,
  kUnsupported,
};

// EndpointServer is a wrapper around fidl::Server<fuchsia_hardware_usb_endpoint::Endpoint> that
// implements common functionality surrounding registering and unregistering VMOs, completing
// requests, etc.
class EndpointServer : public fidl::Server<fuchsia_hardware_usb_endpoint::Endpoint> {
 public:
  EndpointServer(const zx::bti& bti, uint8_t ep_addr, ScatterGatherSupport sg_support)
      : bti_(bti), ep_addr_(ep_addr), sg_support_(sg_support) {}
  virtual ~EndpointServer();

  // Connects to the EndpointServer.
  void Connect(async_dispatcher_t* dispatcher,
               fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end);

  // fuchsia_hardware_usb_new.Endpoint protocol implementation.
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) final;
  void UnregisterVmos(UnregisterVmosRequest& request,
                      UnregisterVmosCompleter::Sync& completer) final;

  // Completes a request.
  void RequestComplete(zx_status_t status, size_t actual, RequestVariant request,
                       std::optional<zx::eventpair> wake_lease = std::nullopt);

  // Gets all the iterators for a request.
  zx::result<std::vector<io_buffer::PhysIter>> get_iter(RequestVariant& req,
                                                        size_t max_length) const;

  const zx::bti& bti() { return bti_; }
  uint8_t ep_addr() const { return ep_addr_; }
  ScatterGatherSupport scatter_gather_support() const { return sg_support_; }

  struct VmoInfo {
    uint64_t id;
    uint64_t size;
  };

  // Gets information about all registered VMOs (ID and size in bytes).
  std::vector<VmoInfo> GetRegisteredVmosInfo() const {
    std::lock_guard<std::mutex> lock(lock_);
    std::vector<VmoInfo> info;
    for (const auto& [id, vmo] : registered_vmos_) {
      info.push_back({id, vmo.size});
    }
    return info;
  }

 protected:
  // OnUnbound: May be overwritten. If not overwritten, unregisters VMOs.
  virtual void OnUnbound(fidl::UnbindInfo info,
                         fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end);

 private:
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_endpoint::Endpoint>> binding_ref_
      __TA_GUARDED(lock_);
  const zx::bti& bti_;
  uint8_t ep_addr_;
  ScatterGatherSupport sg_support_;

  // completions_: Holds on to request completions that are completed, but have not been replied to
  // due to  defer_completion == true.
  std::vector<fuchsia_hardware_usb_endpoint::Completion> completions_ __TA_GUARDED(lock_);

  struct RegisteredVmo {
    zx_handle_t pmt;
    uint64_t* phys_list;
    size_t phys_count;
    uint64_t size;
  };
  // registered_vmos_: All pre-registered VMOs registered through RegisterVmos(). Mapping from
  // vmo_id to RegisteredVmo.
  std::map<fuchsia_hardware_usb_request::VmoId, RegisteredVmo> registered_vmos_ __TA_GUARDED(lock_);

  // Unpins all VMOs in |vmos|, frees physical lists, and removes them from the map.
  // Returns a successful result if all VMOs were unpinned successfully.
  // Otherwise, returns a vector of pairs containing the VMO ID and the error status
  // for each failed unpin operation.
  fit::result<std::vector<std::pair<fuchsia_hardware_usb_request::VmoId, zx_status_t>>> UnpinVmos(
      std::map<fuchsia_hardware_usb_request::VmoId, RegisteredVmo>& vmos);

  mutable std::mutex lock_;
};

}  // namespace usb

#endif  // SRC_DEVICES_USB_LIB_USB_ENDPOINT_INCLUDE_USB_ENDPOINT_USB_ENDPOINT_SERVER_H_
