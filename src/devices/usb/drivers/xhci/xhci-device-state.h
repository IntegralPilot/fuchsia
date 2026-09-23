// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_DEVICE_STATE_H_
#define SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_DEVICE_STATE_H_

#include <lib/inspect/cpp/vmo/types.h>
#include <lib/sync/completion.h>

#include <optional>

#include "src/devices/usb/drivers/xhci/xhci-endpoint.h"
#include "src/devices/usb/drivers/xhci/xhci-hub.h"
#include "zircon/compiler.h"

namespace usb_xhci {

// The maximum number of endpoints a USB device can support
constexpr uint8_t kMaxEndpoints = 32;

// DeviceState is RefCounted. The device itself holds a reference to DeviceState and all its direct
// descendents (if the device is a hub) hold references to DeviceState. DeviceState calls its
// destructor when all references to it is released (see comment in destructor).
// In further detail:
// DeviceState is created when a slot is assigned. A reference is added when a slot is assigned or
// there are any direct descendents that will reference it (In other words, this device is a hub.
// One reference is added per device connected to the hub). A reference is removed when one of its
// descendents is removed or when the hub itself is removed.
class DeviceState : public fbl::RefCounted<DeviceState> {
 public:
  DeviceState(uint32_t device_id, UsbXhci* hci)
      : device_id_(device_id), hci_(hci), tr_(hci, device_id, 0) {}
  ~DeviceState();

  void Disconnect() __TA_REQUIRES(transaction_lock_) { disconnecting_ = true; }

  void SetDeviceInformation(uint8_t slot, uint8_t port, const std::optional<HubInfo>& hub)
      __TA_REQUIRES(transaction_lock_) {
    slot_ = slot;
    port_ = port;
    hub_ = hub;
    disconnecting_ = false;
  }

  // True if the device state has been initialized, false otherwise.
  bool IsValid() const { return slot_; }

  uint8_t GetPort() const { return port_; }

  uint8_t GetSlot() const { return slot_; }

  uint16_t GetInterrupterTarget() const { return interrupter_target_; }

  std::optional<HubInfo>& GetHubLocked() __TA_REQUIRES(transaction_lock_) { return hub_; }

  std::optional<HubInfo>& GetHub() {
    fbl::AutoLock l(&transaction_lock_);
    return hub_;
  }

  bool IsDisconnecting() const __TA_REQUIRES(transaction_lock_) { return disconnecting_; }

  Endpoint& GetEndpoint() __TA_REQUIRES(transaction_lock_) { return tr_; }

  zx_status_t InitEndpoint(uint8_t ep_addr, EventRing* event_ring, fdf::MmioBuffer* mmio)
      __TA_REQUIRES(transaction_lock_);
  // idx should correspond to XhciEndpointIndex
  Endpoint& GetEndpoint(uint8_t idx) __TA_REQUIRES(transaction_lock_) {
    if (!rings_[idx]) {
      rings_[idx].emplace(hci_, device_id_, XhciEndpointIndexInverse(idx + 1));
    }
    return *rings_[idx];
  }

  std::unique_ptr<dma_buffer::PagedBuffer>& GetInputContext() __TA_REQUIRES(transaction_lock_) {
    return input_context_;
  }

  std::unique_ptr<dma_buffer::PagedBuffer>& GetDeviceContext() __TA_REQUIRES(transaction_lock_) {
    return device_context_;
  }

  TRBPromise AddressDeviceCommand(UsbXhci* hci, uint8_t slot_id, uint8_t port_id,
                                  std::optional<HubInfo> hub_info, uint64_t* dcbaa,
                                  uint16_t interrupter_target, CommandRing* command_ring,
                                  fdf::MmioBuffer* mmio, bool bsr);

  fbl::Mutex& transaction_lock() __TA_RETURN_CAPABILITY(transaction_lock_) {
    return transaction_lock_;
  }

  // Used by xhci to break cyclic dependency during dtor.
  void ResetHci() { hci_ = nullptr; }

  void CreateInspectNode(inspect::Node node, uint16_t vendor_id, uint16_t product_id);

  uint32_t device_id() const { return device_id_; }

 private:
  zx_status_t InitializeSlotBuffer(const UsbXhci& hci, uint8_t slot_id, uint8_t port_id,
                                   const std::optional<HubInfo>& hub_info,
                                   std::unique_ptr<dma_buffer::PagedBuffer>* out);

  zx_status_t InitializeEndpointContext(const UsbXhci& hci, uint8_t slot_id, uint8_t port_id,
                                        std::optional<HubInfo>& hub_info,
                                        dma_buffer::PagedBuffer* slot_context_buffer)
      __TA_REQUIRES(transaction_lock_);
  zx_status_t InitializeOutputContextBuffer(const UsbXhci& hci, uint8_t slot_id, uint8_t port_id,
                                            const std::optional<HubInfo>& hub_info, uint64_t* dcbaa,
                                            std::unique_ptr<dma_buffer::PagedBuffer>* out)
      __TA_REQUIRES(transaction_lock_);

  uint32_t device_id_;
  UsbXhci* hci_;
  uint8_t slot_ = 0;
  uint8_t port_ = 0;
  uint16_t interrupter_target_;
  fbl::Mutex transaction_lock_;
  std::optional<HubInfo> hub_ __TA_GUARDED(transaction_lock_);
  bool disconnecting_ __TA_GUARDED(transaction_lock_) = false;
  Endpoint tr_ __TA_GUARDED(transaction_lock_);
  std::optional<Endpoint> rings_[kMaxEndpoints] __TA_GUARDED(transaction_lock_);
  std::unique_ptr<dma_buffer::PagedBuffer> input_context_ __TA_GUARDED(transaction_lock_);
  std::unique_ptr<dma_buffer::PagedBuffer> device_context_ __TA_GUARDED(transaction_lock_);

  // Published inspect data
  inspect::Node inspect_node_;
  inspect::StringProperty vendor_id_;
  inspect::StringProperty product_id_;
};
}  // namespace usb_xhci

#endif  // SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_DEVICE_STATE_H_
