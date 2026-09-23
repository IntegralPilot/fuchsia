// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_VIRTIO_NETDEVICE_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_VIRTIO_NETDEVICE_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/sync/cpp/completion.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <array>
#include <bitset>
#include <memory>
#include <mutex>

#include <fbl/macros.h>
#include <virtio/net.h>

#include "src/connectivity/network/drivers/network-device/device/public/locks.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace virtio {

namespace netdev = fuchsia_hardware_network_driver;

using VmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint32_t>>;

class VirtioNetDriver;

class NetworkDevice : public Device,
                      public fdf::WireServer<netdev::NetworkDeviceImpl>,
                      public fdf::WireServer<netdev::NetworkPort>,
                      public fdf::WireServer<netdev::MacAddr> {
 public:
  // Specifies how many packets can fit in each of the receive and transmit
  // backlogs.
  // Chosen arbitrarily. Larger values will cause increased memory consumption,
  // lower values may cause ring underruns.
  static constexpr uint16_t kMaxDepth = 256;
  // The single port ID created by this device.
  static constexpr uint8_t kPortId = 1;
  // Specifies the maximum transfer unit we support.
  // Picked to mimic common default ethernet frame size.
  static constexpr size_t kMtu = 1514;
  static constexpr size_t kFrameSize = sizeof(virtio_net_hdr_t) + kMtu;

  static constexpr size_t kBufferAlignment = 2048;

  // Queue identifiers.
  static constexpr uint16_t kRxId = 0u;
  static constexpr uint16_t kTxId = 1u;

  static constexpr char kChildNodeName[] = "virtio-net-compat";

  NetworkDevice(VirtioNetDriver* driver, zx::bti bti_handle, std::unique_ptr<Backend> backend,
                const std::shared_ptr<fdf::Namespace>& incoming,
                const std::optional<std::string>& node_name);
  virtual ~NetworkDevice();

  zx_status_t Init() override __TA_EXCLUDES(state_lock_);
  void Shutdown() __TA_EXCLUDES(state_lock_);

  // VirtIO callbacks
  void IrqRingUpdate() override __TA_EXCLUDES(state_lock_);
  void IrqConfigChange() override __TA_EXCLUDES(state_lock_);

  // NetworkDeviceImpl protocol:
  void Init(fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request,
            fdf::Arena& arena, InitCompleter::Sync& completer) override;
  void Start(fdf::Arena& arena, StartCompleter::Sync& completer) override;
  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override;
  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) override;
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

  // NetworkPort protocol:
  void GetInfo(fdf::Arena& arena,
               fdf::WireServer<netdev::NetworkPort>::GetInfoCompleter::Sync& completer) override;
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override;
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override;
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override;
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override;

  // MacAddr protocol:
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

  const char* tag() const override { return "virtio-net"; }

  uint16_t virtio_header_len() const { return virtio_hdr_len_; }

 private:
  friend class NetworkDeviceTests;
  zx_status_t AddDevice();
  void RemoveDevice();

  zx_status_t AckFeatures(bool* is_status_supported, bool* is_multiqueue_supported,
                          uint16_t* virtio_hdr_len);

  DISALLOW_COPY_ASSIGN_AND_MOVE(NetworkDevice);

  // Implementation of IrqRingUpdate; returns true if it should be called again.
  bool IrqRingUpdateInternal() __TA_EXCLUDES(state_lock_);
  fuchsia_hardware_network::PortStatus ReadStatus() const;

  VirtioNetDriver* driver_ = nullptr;

  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> netdev_child_;
  fdf::UnsynchronizedDispatcher netdevice_dispatcher_;
  libsync::Completion netdevice_dispatcher_shutdown_;
  bool irq_thread_started_ = false;

  // Mutexes to control concurrent access
  network::SharedLock state_lock_;
  std::mutex tx_lock_;
  std::mutex rx_lock_;

  // Virtqueues; see section 5.1.2 of the spec
  //
  // This driver doesn't currently support multi-queueing, automatic
  // steering, or the control virtqueue, so only a single queue is needed in
  // each direction.
  //
  // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-1960002
  Ring rx_ __TA_GUARDED(rx_lock_);
  Ring tx_ __TA_GUARDED(tx_lock_);
  uint16_t rx_depth_;
  uint16_t tx_depth_;

  struct Descriptor {
    uint32_t buffer_id;
    uint16_t ring_id;
  };
  class FifoQueue {
   public:
    void Push(Descriptor t) {
      ZX_ASSERT(count_ < data_.size());
      data_[wr_] = t;
      wr_ = (wr_ + 1) % data_.size();
      count_++;
    }
    Descriptor Pop() {
      ZX_ASSERT(count_ > 0);
      Descriptor t = data_[rd_];
      rd_ = (rd_ + 1) % data_.size();
      count_--;
      return t;
    }
    bool Empty() const { return count_ == 0; }

   private:
    std::array<Descriptor, kMaxDepth> data_;
    size_t wr_ = 0;
    size_t rd_ = 0;
    size_t count_ = 0;
  };
  FifoQueue rx_in_flight_ __TA_GUARDED(rx_lock_);
  bool rx_space_requested_ __TA_GUARDED(rx_lock_) = false;

  std::array<uint32_t, kMaxDepth> tx_in_flight_buffer_ids_ __TA_GUARDED(tx_lock_);
  std::bitset<kMaxDepth> tx_in_flight_active_ __TA_GUARDED(tx_lock_);

  // Whether the status field in virtio_net_config is supported.
  bool is_status_supported_;
  // Whether the device supports multiqueue with automatic receive steering.
  bool is_multiqueue_supported_;

  fuchsia_net::wire::MacAddress mac_;
  uint16_t virtio_hdr_len_;

  fdf::WireSharedClient<netdev::NetworkDeviceIfc> ifc_ __TA_GUARDED(state_lock_);

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<std::string> node_name_;
  VmoStore vmo_store_ __TA_GUARDED(state_lock_);
};

}  // namespace virtio

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_VIRTIO_NETDEVICE_H_
