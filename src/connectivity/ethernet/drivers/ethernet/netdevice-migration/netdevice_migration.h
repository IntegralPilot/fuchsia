// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_ETHERNET_NETDEVICE_MIGRATION_NETDEVICE_MIGRATION_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_ETHERNET_NETDEVICE_MIGRATION_NETDEVICE_MIGRATION_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/driver/fidl.h>
#include <fuchsia/hardware/ethernet/cpp/banjo.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/system/public/zircon/compiler.h>

#include <queue>
#include <span>
#include <unordered_set>
#include <utility>

#include <ddktl/device.h>
#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

#include "src/connectivity/network/drivers/network-device/device/public/locks.h"
#include "src/devices/lib/dev-operation/include/lib/operation/ethernet.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace netdevice_migration {

namespace netdev = fuchsia_hardware_network_driver;

using NetdeviceMigrationVmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint32_t>>;
using Netbuf = eth::Operation<uint32_t>;
using NetbufPool = eth::OperationPool<uint32_t>;

class NetdeviceMigration
    : public fdf::DriverBase2,
      public ddk::EthernetIfcProtocol<NetdeviceMigration>,
      public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>,
      public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>,
      public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  static constexpr uint8_t kPortId = 13;
  // Equivalent to old ethernet driver FIFO depth; see
  // https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/ethernet/drivers/ethernet/ethernet.h;l=169;drc=bd653b0d513ea6cc0d2ec85d38ae31bf084f0651
  static constexpr uint32_t kFifoDepth = 256;
  static constexpr uint32_t kMaxBufferSize = 2048;
  static constexpr netdev::wire::SupportedMacFilterMode kSupportedMacFilteringModes =
      netdev::wire::SupportedMacFilterMode::kMulticastFilter |
      netdev::wire::SupportedMacFilterMode::kMulticastPromiscuous |
      netdev::wire::SupportedMacFilterMode::kPromiscuous;
  static constexpr uint32_t kMulticastFilterMax = netdev::wire::kMaxMacFilter;
  static constexpr const char kChildNodeName[] = "netdevice-migration-compat";

  NetdeviceMigration();

  using fdf::DriverBase2::Start;
  using fdf::DriverBase2::Stop;

  // DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }

  // For EthernetIfcProtocol.
  void EthernetIfcStatus(uint32_t status) __TA_EXCLUDES(status_lock_);
  void EthernetIfcRecv(const uint8_t* data_buffer, size_t data_size, uint32_t flags)
      __TA_EXCLUDES(rx_lock_) __TA_EXCLUDES(vmo_lock_);

  // For NetworkDeviceImplProtocol.
  void Init(netdev::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
            InitCompleter::Sync& completer) override;
  void Start(fdf::Arena& arena, StartCompleter::Sync& completer) override;
  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override;
  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) override;
  void QueueTx(netdev::wire::NetworkDeviceImplQueueTxRequest* request, fdf::Arena& arena,
               QueueTxCompleter::Sync& completer) override;
  void QueueRxSpace(netdev::wire::NetworkDeviceImplQueueRxSpaceRequest* request, fdf::Arena& arena,
                    QueueRxSpaceCompleter::Sync& completer) override;
  void PrepareVmo(netdev::wire::NetworkDeviceImplPrepareVmoRequest* request, fdf::Arena& arena,
                  PrepareVmoCompleter::Sync& completer) override;
  void ReleaseVmo(netdev::wire::NetworkDeviceImplReleaseVmoRequest* request, fdf::Arena& arena,
                  ReleaseVmoCompleter::Sync& completer) override;

  // For NetworkPortProtocol.
  void GetInfo(fdf::Arena& arena,
               fdf::WireServer<netdev::NetworkPort>::GetInfoCompleter::Sync& completer) override;
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override;
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override;
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override;
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override;

  // For MacAddrProtocol.
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

 private:
  void SetMacParam(uint32_t param, int32_t value,
                   std::span<const fuchsia_net::wire::MacAddress> data = {}) const;

  zx_status_t DeviceAdd();
  void DeviceRemove();
  void Shutdown();

  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> netdev_child_;

  std::atomic<size_t> no_rx_space_ = 0;

  ddk::EthernetImplProtocolClient ethernet_;
  fdf::WireSharedClient<netdev::NetworkDeviceIfc> netdevice_;
  fdf::UnsynchronizedDispatcher netdevice_dispatcher_;
  libsync::Completion netdevice_dispatcher_shutdown_;
  // This is only used to pass something in the proto ops of DeviceAddArgs. The contents are
  // irrelevant.
  zx_protocol_device_t blank_netdev_ops_{};

  zx::bti eth_bti_;
  netdev::DeviceImplInfo info_;
  uint32_t mtu_;
  std::array<uint8_t, ETH_MAC_SIZE> mac_;
  fuchsia_hardware_network::PortBaseInfo port_info_;
  size_t netbuf_size_;

  std::mutex status_lock_;
  fuchsia_hardware_network::wire::StatusFlags port_status_flags_ __TA_GUARDED(status_lock_);

  std::mutex tx_lock_ __TA_ACQUIRED_AFTER(rx_lock_, vmo_lock_);
  bool tx_started_ __TA_GUARDED(tx_lock_) = false;
  NetbufPool netbuf_pool_ __TA_GUARDED(tx_lock_);
  std::unordered_set<uint32_t> tx_in_flight_ __TA_GUARDED(tx_lock_);

  std::mutex rx_lock_ __TA_ACQUIRED_BEFORE(tx_lock_, vmo_lock_);
  bool rx_started_ __TA_GUARDED(rx_lock_) = false;
  // Use a queue to enforce FIFO ordering. With LIFO ordering, some buffers will sit unused unless
  // the driver hits buffer starvation, which could obscure bugs related to malformed buffers.
  std::queue<netdev::wire::RxSpaceBuffer> rx_spaces_ __TA_GUARDED(rx_lock_);
  bool rx_space_queued_ __TA_GUARDED(rx_lock_) = false;

  network::SharedLock vmo_lock_ __TA_ACQUIRED_BEFORE(tx_lock_) __TA_ACQUIRED_AFTER(rx_lock_);
  std::unique_ptr<NetdeviceMigrationVmoStore> vmo_store_ __TA_GUARDED(vmo_lock_);

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<std::string> node_name_;

  friend class NetdeviceMigrationTestHelper;
};

}  // namespace netdevice_migration

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_ETHERNET_NETDEVICE_MIGRATION_NETDEVICE_MIGRATION_H_
