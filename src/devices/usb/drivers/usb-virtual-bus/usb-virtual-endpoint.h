// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_ENDPOINT_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_ENDPOINT_H_

#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <lib/async/cpp/task.h>

#include <memory>

#include <fbl/mutex.h>
#include <usb/request-cpp.h>
#include <usb/request-fidl.h>

namespace usb_virtual_bus {

static constexpr uint8_t IN_EP_START = 17;
// For mapping b_endpoint_address value to/from index in range 0 - 31.
// OUT endpoints are in range 1 - 15, IN endpoints are in range 17 - 31.
static inline uint8_t EpAddressToIndex(uint8_t addr) {
  return static_cast<uint8_t>(((addr) & 0xF) | (((addr) & 0x80) >> 3));
}

using Request = usb::BorrowedRequest<void>;
using RequestVariant = std::variant<Request, usb::FidlRequest>;

class UsbVirtualEp;
class UsbEpServer : public fidl::Server<fuchsia_hardware_usb_endpoint::Endpoint> {
 public:
  explicit UsbEpServer(UsbVirtualEp* ep) : ep_(ep) {}
  ~UsbEpServer() { ZX_ASSERT(pending_reqs_.empty()); }

  fbl::Mutex& lock() __TA_RETURN_CAPABILITY(lock_) { return lock_; }

  void Connect(fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end);

  void QueueRequest(RequestVariant req);
  void CommonCancelAll();

  void RequestComplete(zx_status_t status, size_t actual, RequestVariant& request);
  zx::result<void*> GetBuffer(RequestVariant& req) __TA_REQUIRES(lock_);

  zx::result<std::optional<usb::MappedVmo>> GetMapped(
      const fuchsia_hardware_usb_request::Buffer& buffer) __TA_REQUIRES(lock_) {
    if (buffer.Which() == fuchsia_hardware_usb_request::Buffer::Tag::kData) {
      return zx::ok(std::nullopt);
    }
    return zx::ok(registered_vmos_.at(buffer.vmo_id().value()));
  }

  struct CurrentRequest {
    RequestVariant req;
    uint8_t* buffer;  // So that it doesn't have to be recalculated every time
    size_t offset = 0;
    const size_t length;  // So that it doesn't have to be recalculated every time

    uint8_t* ptr() { return buffer + offset; }
    size_t todo() const { return length - offset; }
  };
  std::optional<CurrentRequest> current_req_ __TA_GUARDED(lock_);

  // pending_reqs_: Requests not processed yet.
  std::queue<RequestVariant> pending_reqs_ __TA_GUARDED(lock_);

 private:
  // fuchsia_hardware_usb_new.Endpoint protocol implementation.
  void GetInfo(GetInfoCompleter::Sync& completer) override {
    completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
  }
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) override;
  void UnregisterVmos(UnregisterVmosRequest& request,
                      UnregisterVmosCompleter::Sync& completer) override;
  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override;
  void CancelAll(CancelAllCompleter::Sync& completer) override;

  UsbVirtualEp* ep_;
  std::shared_ptr<fidl::ServerBinding<fuchsia_hardware_usb_endpoint::Endpoint>> binding_
      __TA_GUARDED(lock_);

  // completions_: Holds on to request completions that are completed, but have not been replied
  // to due to  defer_completion == true.
  std::vector<fuchsia_hardware_usb_endpoint::Completion> completions_ __TA_GUARDED(lock_);

  // registered_vmos_: All pre-registered VMOs registered through RegisterVmos(). Mapping from
  // vmo_id to usb::MappedVmo.
  std::map<uint64_t, usb::MappedVmo> registered_vmos_ __TA_GUARDED(lock_);

  fbl::Mutex lock_;
};

class UsbVirtualBus;
// This class represents an endpoint on the virtual device.
class UsbVirtualEp {
 public:
  void Init(UsbVirtualBus* bus, uint8_t index) {
    bus_ = bus;
    index_ = index;
  }

  void ProcessRequests();
  void HandleControl(RequestVariant req);
  zx::result<> SetStall(bool stall);

  bool is_control() const { return index_ == 0; }
  bool is_out() const { return index_ > 0 && index_ < IN_EP_START; }
  bool is_in() const { return !is_control() && !is_out(); }

  UsbVirtualBus* bus_;
  uint8_t index_;

  uint16_t max_packet_size_ = 0;
  bool stalled_ = false;

  UsbEpServer host_{this};
  UsbEpServer device_{this};

  async::TaskClosureMethod<UsbVirtualEp, &UsbVirtualEp::ProcessRequests> process_requests_{this};
};

}  // namespace usb_virtual_bus

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_ENDPOINT_H_
