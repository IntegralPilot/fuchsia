// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_BIN_VMM_DEVICE_VIRTIO_NET_SRC_CPP_GUEST_ETHERNET_H_
#define SRC_VIRTUALIZATION_BIN_VMM_DEVICE_VIRTIO_NET_SRC_CPP_GUEST_ETHERNET_H_

// Don't reorder this header to avoid conflicting implementations of MAX_PORTS.
// clang-format off
#include <fuchsia/net/virtualization/cpp/fidl.h>
// clang-format on

#include <fidl/fuchsia.hardware.network.driver/cpp/wire.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <fuchsia/net/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/trace-provider/provider.h>
#include <zircon/types.h>

#include <virtio/net.h>

#include "src/connectivity/network/drivers/network-device/device/public/network_device.h"
#include "src/virtualization/bin/vmm/device/virtio_net/src/cpp/completion_queue.h"

class GuestEthernet : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>,
                      public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr>,
                      public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort> {
 public:
  GuestEthernet(fdf::Dispatcher* sync_dispatcher,
                const network::DeviceInterfaceDispatchers& netdev_dispatchers);
  ~GuestEthernet();

  // Initializes this guest ethernet object by parsing the Rust provided MAC address, preparing
  // callbacks, and registering it the netstack. This will be invoked by the Rust thread, and
  // scheduled on the C++ dispatch loop.
  //
  // Returns ZX_OK if it was successfully scheduled, and sends ZX_OK via set_status_ when finished.
  zx_status_t Initialize(const void* rust_guest_ethernet, const uint8_t* mac, size_t mac_len,
                         bool enable_bridge);

  // Send the packet to the netstack, returning ZX_OK if the packet was sent successfully, and
  // ZX_ERR_SHOULD_WAIT if no buffer space is available and the device should retry later.
  zx_status_t Send(const void* data, uint16_t length);

  // Indicate that a packet has been successfully sent to the guest and that the memory can be
  // reclaimed.
  void Complete(uint32_t buffer_id, zx_status_t status);

  fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl> BindDriver();

  // Methods implementing the `NetworkDevice` protocol.
  void Init(fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request,
            fdf::Arena& arena, InitCompleter::Sync& completer) override;
  void Start(fdf::Arena& arena, StartCompleter::Sync& completer) override;
  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override;
  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>::GetInfoCompleter::Sync&
          completer) override;
  void QueueTx(fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
               fdf::Arena& arena, QueueTxCompleter::Sync& completer) override;
  void QueueRxSpace(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
      fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) override;
  void PrepareVmo(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
      fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) override;
  void ReleaseVmo(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
      fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) override;

  // Methods implementing the `MacAddr` protocol.
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

  // Methods implementing the `NetworkPort` protocol.
  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>::GetInfoCompleter::Sync&
          completer) override;
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override;
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override;
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override;
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override;

  // Port GuestEthernet uses for communication.
  static constexpr uint8_t kPortId = 0;

 private:
  enum class State {
    kStopped,       // Device is idle.
    kStarted,       // Device has started.
    kShuttingDown,  // Device is shutting down, waiting for outstanding transmissions to complete.
  };

  void Teardown();

  // Notify the netstack that the given buffer has been used. A length of 0 can be sent to indicate
  // that the buffer was unused.
  //
  // Note that this is tx from the perspective of the guest.
  void TxComplete(uint32_t buffer_id, size_t length);

  // Notify the netstack that the buffer has been sent to the guest (or failed, depending on the
  // status). As soon as this function is invoked, the netstack is free to reuse the underlying
  // buffer memory.
  //
  // Note that this is rx from the perspective of the guest.
  void RxComplete(uint32_t buffer_id, zx_status_t status);

  // Register this guest ethernet object with the netstack.
  zx_status_t CreateGuestInterface(bool enable_bridge);

  // If in state kShuttingDown with no in flight RX to the guest, this will invoke the shutdown
  // complete callback.
  void FinishShutdownIfRequired() __TA_REQUIRES(mutex_);

  // Return a span of memory inside the VMO. Returns nullopt if the given range is invalid.
  zx::result<cpp20::span<uint8_t>> GetIoRegion(uint8_t vmo_id, uint64_t offset, uint64_t length)
      __TA_REQUIRES(mutex_);

  fuchsia_hardware_network::PortStatus GetPortStatus();

  std::mutex mutex_;
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> parent_;

  // Device state.
  State state_ __TA_GUARDED(mutex_) = State::kStopped;
  uint32_t in_flight_rx_ __TA_GUARDED(mutex_);  // Packets sent to the guest but uncompleted.
  fit::function<void()> shutdown_complete_callback_ __TA_GUARDED(mutex_);

  // Memory shared with netstack.
  zx::vmo io_vmo_ __TA_GUARDED(mutex_);  // VMO shared with netstack for packet transfer.
  uint8_t* io_addr_ __TA_GUARDED(mutex_) = nullptr;     // Beginning of the IO region.
  size_t io_size_ __TA_GUARDED(mutex_) = 0;             // Length of the mapping, in bytes.
  std::optional<uint8_t> vmo_id_ __TA_GUARDED(mutex_);  // Netstack's identifier for the VMO.

  // Available buffers for sending packets to netstack.
  struct AvailableBuffer {
    uint32_t buffer_id;
    cpp20::span<uint8_t> region;
  };
  std::vector<AvailableBuffer> available_buffers_ __TA_GUARDED(mutex_);

  const fdf::Dispatcher* sync_dispatcher_;
  const network::DeviceInterfaceDispatchers netdev_dispatchers_;
  trace::TraceProviderWithFdio trace_provider_;
  std::shared_ptr<sys::ServiceDirectory> svc_;

  GuestToHostCompletionQueue tx_completion_queue_;
  HostToGuestCompletionQueue rx_completion_queue_;

  uint8_t mac_address_[VIRTIO_ETH_MAC_SIZE];

  ::fuchsia::net::virtualization::ControlPtr netstack_;
  ::fuchsia::net::virtualization::NetworkPtr network_;
  ::fuchsia::net::virtualization::InterfacePtr interface_registration_;
  ::std::unique_ptr<network::NetworkDeviceInterface> device_interface_;

  fit::function<void()> ready_for_guest_tx_;
  fit::function<void(zx_status_t)> set_status_;
  fit::function<void(uint8_t*, size_t, uint32_t)> send_guest_rx_;
};

#endif  // SRC_VIRTUALIZATION_BIN_VMM_DEVICE_VIRTIO_NET_SRC_CPP_GUEST_ETHERNET_H_
