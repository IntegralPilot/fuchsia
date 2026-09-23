// Copyright (c) 2022 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_THIRD_PARTY_IGC_IGC_DRIVER_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_THIRD_PARTY_IGC_IGC_DRIVER_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <lib/async/cpp/irq.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <zircon/types.h>

#include <mutex>

#include "igc_api.h"
#include "src/connectivity/network/drivers/network-device/device/public/locks.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace ethernet {
namespace igc {

using VmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint32_t>>;

// Internal Rx/Tx buffer metadata.
constexpr static size_t kEthRxBufCount =
    256;  // Depth of Rx buffer, equal to the length of Rx desc ring buffer.
constexpr static size_t kEthRxBufSize = 2048;  // Size of each Rx frame buffer.
constexpr static size_t kEthRxDescSize = 16;

constexpr static size_t kEthTxDescRingCount =
    256;  // The length of Tx desc ring buffer pool, should be greater than the actual tx buffer
          // count, which left dummy spaces for ring buffer indexing. Both tx desc ring buffer pool
          // length and actual tx buffer count should be multiples of 2.

constexpr static size_t kEthTxBufCount =
    128;  // Depth of Tx buffer that driver reports to network device.
constexpr static size_t kEthTxBufSize = 2048;  // Size of each Tx frame buffer.
constexpr static size_t kEthTxDescSize = 16;

static constexpr size_t kEthRxTxMaxBufSize = 2048;
static constexpr size_t kEthRxTxBufAlignment = 2048;

constexpr static uint8_t kPortId = 1;
constexpr static size_t kEtherMtu = 1500;
constexpr static size_t kEtherAddrLen = 6;

#define USE_NETDEV_DISPATCHER

class IgcDriver final : public fdf::DriverBase2,
                        public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>,
                        public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>,
                        public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  struct buffer_info;
  struct adapter;

  IgcDriver();
  ~IgcDriver() override;

  // DriverBase2 implementation.
  using DriverBase2::Start;
  zx::result<> Start(fdf::DriverContext context) override;
  using DriverBase2::Stop;
  void Stop(fdf::StopCompleter completer) override;

  // NetworkDeviceImpl implementation
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

  // NetworkPort protocol implementation.
  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>::GetInfoCompleter::Sync&
          completer) override;
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

  buffer_info* RxBuffer() { return rx_buffers_; }
  buffer_info* TxBuffer() { return tx_buffers_; }
  std::shared_ptr<adapter> Adapter() { return adapter_; }

  // The return value indicates whether the online status has been changed.
  bool OnlineStatusUpdate();

  struct adapter {
    struct igc_hw hw;
    struct igc_osdep osdep;
    mtx_t lock;
    zx_handle_t irqh;
    zx_handle_t btih;

    // Buffer to store tx/rx descriptor rings.
    io_buffer_t desc_buffer;

    // tx/rx descriptor rings
    struct igc_tx_desc* txdr;
    union igc_adv_rx_desc* rxdr;

    // Base physical addresses for tx/rx rings.
    // Store as 64-bit integer to match hw registers sizes.
    uint64_t txd_phys;
    uint64_t rxd_phys;

    // callback interface to attached ethernet layer
    fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> netdev_ifc;

    uint32_t txh_ind;  // Index of the head of awaiting(for the adapter to pick up) buffers in txdr.
    uint32_t txt_ind;  // Index of the tail of awaiting(for the adapter to pick up) buffers in txdr.
    uint32_t txr_len;  // Number of current entries in txdr.
    uint32_t rxh_ind;  // Index of the head of available buffers in rxdr.
    uint32_t rxt_ind;  // Index of the tail of available buffers in rxdr.
    uint32_t rxr_len;  // Number of current entries in rxdr.
    // Protect the rx data path between the two operations: QueueRxSpace and IgcIrqThreadFunc.
    std::mutex rx_lock;
    // Protect the tx data path between the two operations: QueueTx and ReapTxBuffers.
    std::mutex tx_lock;
    fuchsia_hardware_pci::InterruptMode irq_mode;

    // Indicate a size for the arena so that we avoid re-allocation as much as possible. Don't use
    // this arena for anything other than these RX buffers.
    static constexpr size_t kArenaSize = fidl::MaxSizeInChannel<
        fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest,
        fidl::MessageDirection::kSending>();
    fidl::Arena<kArenaSize> rx_buffer_arena;

    // Allocate a buffer array at half of the rx buffer ring size, and this limits the maximum
    // number of packets that the driver passes up in one batch.
    static constexpr size_t kRxBuffersPerBatch = kEthRxBufCount / 2;
    fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer> buffers{rx_buffer_arena,
                                                                              kRxBuffersPerBatch};
  };

  // We store the buffer information for each rx_space_buffer in this struct, so that we can do
  // CompleteRx based on these information.
  struct buffer_info {
    uint32_t buffer_id;
    bool available;  // Indicate whether this buffer info maps to a buffer passed down through
                     // RxSpace.
  };

 private:
  void Shutdown();
  zx_status_t ConfigurePci(fdf::Namespace& incoming);
  zx_status_t Initialize(fdf::DriverContext& context, std::shared_ptr<fdf::Namespace> incoming);
  zx_status_t AddNetworkDevice();
  bool IsValidEthernetAddr(uint8_t* addr);
  void IdentifyHardware();
  zx_status_t AllocatePCIResources();
  zx_status_t InitBuffer();
  void InitTransmitUnit();
  void InitReceiveUnit();
  void IfSetPromisc(uint32_t flags);
  void ReapTxBuffers();

  void EnableInterrupt();
  void DisableInterrupt();
  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq_base, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);

  std::shared_ptr<struct adapter> adapter_{std::make_shared<struct adapter>()};
  async::IrqMethod<IgcDriver, &IgcDriver::HandleIrq> irq_handler_{this};

  enum class State { Running, ShuttingDown, ShutDown };

  std::atomic<State> state_ = State::Running;

  std::atomic<bool> started_ = false;
  std::atomic<bool> online_ = false;

  // Lock for VMO changes.
  network::SharedLock vmo_lock_;

  // Note: Extract Tx/Rx related variables to pcie_txrx.{h,cc}.
  // The io_buffers created from the pre-allocated VMOs.
  std::unique_ptr<VmoStore> vmo_store_ __TA_GUARDED(vmo_lock_);

  // An extension for rx decriptor.
  buffer_info rx_buffers_[kEthRxBufCount]{};

  // An extension for tx decriptor.
  buffer_info tx_buffers_[kEthTxDescRingCount]{};

  fdf::UnownedSynchronizedDispatcher dispatcher_;

  fdf::UnsynchronizedDispatcher netdev_dispatcher_;

  // Because NetworkDevice is currently DFv1 we need to have a compatibility server in place.
  compat::SyncInitializedDeviceServer compat_server_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::NetworkDeviceImpl>>
      netdev_binding_;
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::NetworkPort>> port_binding_;
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::MacAddr>> mac_binding_;
};

}  // namespace igc
}  // namespace ethernet

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_THIRD_PARTY_IGC_IGC_DRIVER_H_
