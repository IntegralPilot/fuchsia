// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_DWMAC_DWMAC_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_DWMAC_DWMAC_H_

#include <fidl/fuchsia.hardware.ethernet.board/cpp/wire.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <fuchsia/hardware/ethernet/cpp/banjo.h>
#include <fuchsia/hardware/ethernet/mac/cpp/banjo.h>
#include <lib/ddk/device.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/vmo.h>
#include <threads.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <atomic>
#include <bitset>
#include <memory>
#include <optional>

#include <ddktl/device.h>

#include "dw-gmac-dma.h"
#include "pinned-buffer.h"
#include "src/connectivity/network/drivers/network-device/device/public/locks.h"
#include "src/lib/vmo_store/vmo_store.h"

// clang-format off
#define DW_MAC_MAC_CONF             (0x0000)
#define DW_MAC_MAC_FRAMEFILT        (0x0004)
#define DW_MAC_MAC_HASHTABLEHI      (0x0008)
#define DW_MAC_MAC_HASHTABLELO      (0x000c)
#define DW_MAC_MAC_MIIADDR          (0x0010)
#define DW_MAC_MAC_MIIDATA          (0x0014)
#define DW_MAC_MAC_FLOWCONTROL      (0x0018)
#define DW_MAC_MAC_VALANTAG         (0x001c)
#define DW_MAC_MAC_VERSION          (0x0020)
#define DW_MAC_MAC_DEBUG            (0x0024)
#define DW_MAC_MAC_REMOTEWAKEFILT   (0x0028)
#define DW_MAC_MAC_PMTCONTROL       (0x002c)
#define DW_MAC_MAC_LPICONTROL       (0x0030)
#define DW_MAC_MAC_LPITIMERS        (0x0034)
#define DW_MAC_MAC_INTREG           (0x0038)
#define DW_MAC_MAC_INTMASK          (0x003c)
#define DW_MAC_MAC_MACADDR0HI       (0x0040)
#define DW_MAC_MAC_MACADDR0LO       (0x0044)
#define DW_MAC_MAC_MACADDR1HI       (0x0048)
#define DW_MAC_MAC_MACADDR1LO       (0x004c)
#define DW_MAC_MAC_RGMIISTATUS      (0x00d8)

// Offsets of the mac management counters
#define DW_MAC_MMC_CNTRL            (0x0100)
#define DW_MAC_MMC_INTR_RX          (0x0104)
#define DW_MAC_MMC_INTR_TX          (0x0108)
#define DW_MAC_MMC_INTR_MASK_RX     (0x010c)
#define DW_MAC_MMC_INTR_MASK_TX     (0x0110)
#define DW_MAC_MMC_RXFRAMECOUNT_GB  (0x0180)
#define DW_MAC_MMC_RXOCTETCOUNT_GB  (0x0184)
#define DW_MAC_MMC_RXOCTETCOUNT_G   (0x0188)
#define DW_MAC_MMC_IPC_INTR_MASK_RX (0x0200)
#define DW_MAC_MMC_IPC_INTR_RX      (0x0208)

// Offsets of DMA registers
#define DW_MAC_DMA_BUSMODE              (0x1000)
#define DW_MAC_DMA_TXPOLLDEMAND         (0x1004)
#define DW_MAC_DMA_RXPOLLDEMAND         (0x1008)
#define DW_MAC_DMA_RXDESCLISTADDR       (0x100c)
#define DW_MAC_DMA_TXDESCLISTADDR       (0x1010)
#define DW_MAC_DMA_STATUS               (0x1014)
#define DW_MAC_DMA_OPMODE               (0x1018)
#define DW_MAC_DMA_INTENABLE            (0x101c)
#define DW_MAC_DMA_MISSEDFRAMES         (0x1020)
#define DW_MAC_DMA_RXWDT                (0x1024)
#define DW_MAC_DMA_AXIBUSMODE           (0x1028)
#define DW_MAC_DMA_AXISTATUS            (0x102c)
#define DW_MAC_DMA_CURRHOSTTXDESC       (0x1048)
#define DW_MAC_DMA_CURRHOSTRXDESC       (0x104c)
#define DW_MAC_DMA_CURRHOSTTXBUFFADDR   (0x1050)
#define DW_MAC_DMA_CURRHOSTRXBUFFADDR   (0x1054)
#define DW_MAC_DMA_HWFEATURE            (0x1058)

//DMA transaction descriptors
typedef volatile struct dw_dmadescr {
    uint32_t txrx_status;
    uint32_t dmamac_cntl;
    uint32_t dmamac_addr;
    uint32_t dmamac_next;
} __ALIGNED(64) dw_dmadescr_t;
// clang-format on

namespace eth {

class NetworkFunction;
class EthMacFunction;

namespace netdev = fuchsia_hardware_network_driver;

class DWMacDevice : public ddk::Device<DWMacDevice, ddk::Unbindable, ddk::Suspendable>

{
 public:
  static constexpr uint8_t kPortId = 13;
  static constexpr size_t kMtu = MAC_MAX_FRAME_SZ;

  DWMacDevice(zx_device_t* device, fdf::PDev pdev, zx::bti bti,
              fidl::ClientEnd<fuchsia_hardware_ethernet_board::EthBoard> eth_board);

  static zx_status_t Create(void* ctx, zx_device_t* device);

  void DdkRelease();
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkSuspend(ddk::SuspendTxn txn);

  // ZX_PROTOCOL_ETH_MAC ops.
  zx_status_t EthMacMdioWrite(uint32_t reg, uint32_t val);
  zx_status_t EthMacMdioRead(uint32_t reg, uint32_t* val);
  zx_status_t EthMacRegisterCallbacks(const eth_mac_callbacks_t* callbacks);

  // For NetworkDeviceImplProtocol.
  void NetworkDeviceImplInit(
      netdev::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::InitCompleter::Sync& completer);
  void NetworkDeviceImplStart(
      fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::StartCompleter::Sync& completer);
  void NetworkDeviceImplStop(
      fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::StopCompleter::Sync& completer);
  void NetworkDeviceImplGetInfo(
      fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer);
  void NetworkDeviceImplQueueTx(
      netdev::wire::NetworkDeviceImplQueueTxRequest* request, fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::QueueTxCompleter::Sync& completer);
  void NetworkDeviceImplQueueRxSpace(
      netdev::wire::NetworkDeviceImplQueueRxSpaceRequest* request, fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::QueueRxSpaceCompleter::Sync& completer);
  void NetworkDeviceImplPrepareVmo(
      netdev::wire::NetworkDeviceImplPrepareVmoRequest* request, fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::PrepareVmoCompleter::Sync& completer);
  void NetworkDeviceImplReleaseVmo(
      netdev::wire::NetworkDeviceImplReleaseVmoRequest* request, fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::ReleaseVmoCompleter::Sync& completer);

  // For NetworkPortProtocol.
  void NetworkPortGetInfo(fdf::Arena& arena,
                          fdf::WireServer<netdev::NetworkPort>::GetInfoCompleter::Sync& completer);
  void NetworkPortGetStatus(
      fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::GetStatusCompleter::Sync& completer);
  void NetworkPortSetActive(
      fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
      fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::SetActiveCompleter::Sync& completer);
  void NetworkPortGetMac(fdf::Arena& arena,
                         fdf::WireServer<netdev::NetworkPort>::GetMacCompleter::Sync& completer);
  void NetworkPortRemoved(fdf::Arena& arena,
                          fdf::WireServer<netdev::NetworkPort>::RemovedCompleter::Sync& completer);

  // For MacAddrProtocol.
  void MacAddrGetAddress(fdf::Arena& arena,
                         fdf::WireServer<netdev::MacAddr>::GetAddressCompleter::Sync& completer);
  void MacAddrGetFeatures(fdf::Arena& arena,
                          fdf::WireServer<netdev::MacAddr>::GetFeaturesCompleter::Sync& completer);
  void MacAddrSetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
                      fdf::Arena& arena,
                      fdf::WireServer<netdev::MacAddr>::SetModeCompleter::Sync& completer);

 private:
  friend class EthMacFunction;
  friend class NetworkFunction;

  zx_status_t AllocateBuffers();
  zx_status_t InitBuffers();
  zx_status_t InitDevice();
  zx_status_t DeInitDevice() __TA_REQUIRES(state_lock_);
  zx_status_t InitPdev();
  zx_status_t ShutDown();

  void UpdateLinkStatus() __TA_REQUIRES(state_lock_);
  void SendPortStatus() __TA_REQUIRES(state_lock_);
  void DumpRegisters();
  void DumpStatus(uint32_t status);
  void ReleaseBuffers() __TA_REQUIRES(state_lock_);
  void ProcRxBuffer(uint32_t int_status) __TA_REQUIRES_SHARED(state_lock_);
  void ProcTxBuffer() __TA_REQUIRES_SHARED(state_lock_);
  uint32_t DmaRxStatus();

  int Thread();
  int WorkerThread();

  zx_status_t GetMAC(zx_device_t* dev);

  // Number each of tx/rx transaction descriptors
  //  2048 buffers = ~24ms of packets
  static constexpr uint32_t kNumDesc = 2048;

  network::SharedLock state_lock_;
  std::mutex tx_lock_;
  std::mutex rx_lock_;

  dw_dmadescr_t* tx_descriptors_ __TA_GUARDED(tx_lock_) = nullptr;
  dw_dmadescr_t* rx_descriptors_ __TA_GUARDED(rx_lock_) = nullptr;

  fbl::RefPtr<PinnedBuffer> desc_buffer_;

  uint32_t tx_head_ __TA_GUARDED(tx_lock_) = 0;
  uint32_t tx_tail_ __TA_GUARDED(tx_lock_) = 0;
  uint32_t rx_head_ __TA_GUARDED(rx_lock_) = 0;
  uint32_t rx_tail_ __TA_GUARDED(rx_lock_) = 0;
  uint32_t rx_queued_ __TA_GUARDED(rx_lock_) = 0;

  // ethermac fields
  std::array<uint8_t, 6> mac_ __TA_GUARDED(state_lock_) = {};
  uint16_t mii_addr_ = 0;

  const zx::bti bti_;
  zx::interrupt dma_irq_;

  fdf::PDev pdev_;
  fidl::WireSyncClient<fuchsia_hardware_ethernet_board::EthBoard> eth_board_;

  std::optional<fdf::MmioBuffer> mmio_;

  std::optional<fdf::OutgoingDirectory> outgoing_dir_;
  fdf::SynchronizedDispatcher outgoing_dispatcher_;
  libsync::Completion outgoing_dispatcher_shutdown_;
  fdf::UnsynchronizedDispatcher netdev_dispatcher_;
  libsync::Completion netdev_dispatcher_shutdown_;
  fdf::WireSharedClient<netdev::NetworkDeviceIfc> netdevice_ __TA_GUARDED(state_lock_);
  bool started_ __TA_GUARDED(state_lock_) = false;

  bool online_ __TA_GUARDED(state_lock_) = false;

  // statistics
  std::atomic<uint32_t> bus_errors_ = 0;
  std::atomic<uint32_t> tx_counter_ = 0;
  std::atomic<uint32_t> rx_packet_ = 0;
  std::atomic<uint32_t> loop_count_ = 0;

  std::atomic<bool> running_;

  using VmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint32_t>>;
  VmoStore vmo_store_ __TA_GUARDED(state_lock_);

  std::array<netdev::wire::TxResult, kNumDesc> tx_in_flight_buffer_ids_ __TA_GUARDED(tx_lock_);
  std::array<std::pair<uint32_t, void*>, kNumDesc> rx_in_flight_buffer_ids_ __TA_GUARDED(rx_lock_);
  std::bitset<kNumDesc> tx_in_flight_active_ __TA_GUARDED(tx_lock_);

  thrd_t thread_;
  thrd_t worker_thread_;

  // PHY callbacks.
  eth_mac_callbacks_t cbs_;

  // Callbacks registered signal.
  sync_completion_t cb_registered_signal_;

  NetworkFunction* network_function_;
  EthMacFunction* mac_function_;
};

class NetworkFunction : public ddk::Device<NetworkFunction, ddk::Unbindable, ddk::Suspendable>,
                        public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>,
                        public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>,
                        public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  explicit NetworkFunction(zx_device_t* device, DWMacDevice* peripheral)
      : ddk::Device<NetworkFunction, ddk::Unbindable, ddk::Suspendable>(device),
        device_(peripheral) {}

  void DdkUnbind(ddk::UnbindTxn txn) {
    device_->mac_function_ = nullptr;
    device_ = nullptr;
    txn.Reply();
  }
  void DdkSuspend(ddk::SuspendTxn txn) {
    device_->mac_function_ = nullptr;
    device_ = nullptr;
    txn.Reply(ZX_OK, txn.requested_state());
  }
  void DdkRelease() {
    ZX_ASSERT(device_ == nullptr);
    delete this;
  }

  // For NetworkDeviceImplProtocol.
  void Init(fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request,
            fdf::Arena& arena, InitCompleter::Sync& completer) override {
    device_->NetworkDeviceImplInit(request, arena, completer);
  }
  void Start(fdf::Arena& arena, StartCompleter::Sync& completer) override {
    device_->NetworkDeviceImplStart(arena, completer);
  }
  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override {
    device_->NetworkDeviceImplStop(arena, completer);
  }
  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<netdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) override {
    device_->NetworkDeviceImplGetInfo(arena, completer);
  }
  void QueueTx(fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
               fdf::Arena& arena, QueueTxCompleter::Sync& completer) override {
    device_->NetworkDeviceImplQueueTx(request, arena, completer);
  }
  void QueueRxSpace(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
      fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) override {
    device_->NetworkDeviceImplQueueRxSpace(request, arena, completer);
  }
  void PrepareVmo(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
      fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) override {
    device_->NetworkDeviceImplPrepareVmo(request, arena, completer);
  }
  void ReleaseVmo(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
      fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) override {
    device_->NetworkDeviceImplReleaseVmo(request, arena, completer);
  }

  // For NetworkPortProtocol.
  void GetInfo(fdf::Arena& arena,
               fdf::WireServer<netdev::NetworkPort>::GetInfoCompleter::Sync& completer) override {
    device_->NetworkPortGetInfo(arena, completer);
  }
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override {
    device_->NetworkPortGetStatus(arena, completer);
  }
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override {
    device_->NetworkPortSetActive(request, arena, completer);
  }
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override {
    device_->NetworkPortGetMac(arena, completer);
  }
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override {
    device_->NetworkPortRemoved(arena, completer);
  }

  // For MacAddrProtocol.
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override {
    device_->MacAddrGetAddress(arena, completer);
  }
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override {
    device_->MacAddrGetFeatures(arena, completer);
  }
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override {
    device_->MacAddrSetMode(request, arena, completer);
  }

  zx_status_t Add(const char* name, fdf_dispatcher_t* dispatcher,
                  fdf::OutgoingDirectory& outgoing_dir);

 private:
  friend DWMacDevice;
  DWMacDevice* device_;
};

using EthMacFunctionType = ddk::Device<EthMacFunction, ddk::Unbindable, ddk::Suspendable>;
class EthMacFunction : public EthMacFunctionType,
                       public ddk::EthMacProtocol<EthMacFunction, ddk::base_protocol> {
 public:
  explicit EthMacFunction(zx_device_t* device, DWMacDevice* peripheral)
      : EthMacFunctionType(device), device_(peripheral) {}

  void DdkUnbind(ddk::UnbindTxn txn) {
    device_->mac_function_ = nullptr;
    device_ = nullptr;
    txn.Reply();
  }
  void DdkSuspend(ddk::SuspendTxn txn) {
    device_->mac_function_ = nullptr;
    device_ = nullptr;
    txn.Reply(ZX_OK, txn.requested_state());
  }
  void DdkRelease() {
    ZX_ASSERT(device_ == nullptr);
    delete this;
  }

  // ZX_PROTOCOL_ETH_MAC ops.
  zx_status_t EthMacMdioWrite(uint32_t reg, uint32_t val) {
    return device_->EthMacMdioWrite(reg, val);
  }
  zx_status_t EthMacMdioRead(uint32_t reg, uint32_t* val) {
    return device_->EthMacMdioRead(reg, val);
  }
  zx_status_t EthMacRegisterCallbacks(const eth_mac_callbacks_t* callbacks) {
    return device_->EthMacRegisterCallbacks(callbacks);
  }

 private:
  DWMacDevice* device_;
};

}  // namespace eth

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_DWMAC_DWMAC_H_
