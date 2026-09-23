// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_FUCHSIA_H_
#define ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_FUCHSIA_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <lib/async/cpp/irq.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <zircon/types.h>

#include "adapter.h"
#include "rings.h"
#include "src/connectivity/network/drivers/network-device/device/public/locks.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace e1000 {

constexpr uint8_t kPortId = 0;
constexpr uint32_t kMaxMulticastFilters = 128;

class DeviceBase;
struct adapter;

class Driver : public fdf::DriverBase2 {
 public:
  Driver();

  const adapter* Adapter() const;
  adapter* Adapter();

  // DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

 private:
  friend class DeviceBase;

  std::unique_ptr<DeviceBase> device_;

  std::shared_ptr<fdf::Namespace> incoming_;
  std::string node_name_;
};

// Provide access to DriverBase base class and allow storing the Device in a unique_ptr without
// specifying a template parameter.
class DeviceBase {
 public:
  explicit DeviceBase(Driver& driver) : driver_(driver) {}
  virtual ~DeviceBase() = default;

  virtual zx::result<> Start() = 0;
  virtual void Stop(fdf::StopCompleter completer) = 0;

  virtual const adapter* Adapter() const = 0;
  virtual adapter* Adapter() = 0;

 protected:
  // Forward some calls to allow implementations access to fdf::DriverBase2
  const fidl::ClientEnd<fuchsia_driver_framework::Node>& node() const { return driver_.node(); }
  const std::shared_ptr<fdf::Namespace>& incoming() const { return driver_.incoming_; }
  const std::string& node_name() const { return driver_.node_name_; }
  std::string_view name() const { return driver_.name(); }
  std::shared_ptr<fdf::OutgoingDirectory>& outgoing() { return driver_.outgoing(); }
  const fdf::UnownedSynchronizedDispatcher& driver_dispatcher() const {
    return driver_.driver_dispatcher();
  }
  async_dispatcher_t* dispatcher() const { return driver_.dispatcher(); }

 private:
  Driver& driver_;
};

// There are potential assumptions in this code about rx descriptors all being the same size. If
// this were to change for any reason we should put some effort into investigating the implications.
static_assert(sizeof(e1000_rx_desc) == sizeof(e1000_rx_desc_extended) &&
                  sizeof(e1000_rx_desc) == sizeof(e1000_adv_rx_desc),
              "This code assumes that all RX descriptors are the same size.");

// This is a templated implementation of the driver to allow different behavior for different
// generations of chips without resorting to virtual calls or function pointers.
template <typename RxDescriptor>
class Device : public DeviceBase,
               public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>,
               public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>,
               public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  Device(Driver& driver, std::unique_ptr<adapter>&& adapter,
         std::unique_ptr<compat::SyncInitializedDeviceServer>&& compat_server);

  zx::result<> Start() override;
  void Stop(fdf::StopCompleter completer) override;

  const adapter* Adapter() const override { return adapter_.get(); }
  adapter* Adapter() override { return adapter_.get(); }

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

  // MacAddr protocol implementation.
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

 private:
  zx::result<> AllocatePciResources();
  zx::result<> SetupDescriptors();
  zx::result<> AddNetworkDevice();
  void InitializeTransmitUnit() __TA_REQUIRES(adapter_->tx_mutex);
  void InitializeReceiveUnit() __TA_REQUIRES(adapter_->rx_mutex);
  void EnableInterrupts();
  void DisableInterrupts();

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq_base, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);
  void FlushTxQueue() __TA_REQUIRES(adapter_->tx_mutex);
  void FlushRxSpace() __TA_REQUIRES(adapter_->rx_mutex);
  void UpdateOnlineStatus(bool reset);
  void OnLinkStateChange(async_dispatcher_t*, async::TaskBase*, zx_status_t);
  void SendOnlineStatus(bool online);
  void SetMacFilterMode() __TA_REQUIRES(mac_filter_mutex_);
  zx_paddr_t GetPhysicalAddress(const fuchsia_hardware_network_driver::wire::BufferRegion& region)
      __TA_REQUIRES_SHARED(vmo_lock_);

  std::unique_ptr<adapter> adapter_;

  std::atomic<bool> started_ = false;
  std::atomic<bool> online_ = false;

  RxRing<kRxDepth, RxDescriptor> rx_ring_ __TA_GUARDED(adapter_->rx_mutex);
  TxRing<kTxDepth> tx_ring_ __TA_GUARDED(adapter_->tx_mutex);

  fuchsia_hardware_network::wire::MacFilterMode mac_filter_mode_ __TA_GUARDED(mac_filter_mutex_) =
      fuchsia_hardware_network::wire::MacFilterMode::kPromiscuous;
  std::vector<uint8_t> mac_filters_ __TA_GUARDED(mac_filter_mutex_);
  std::mutex mac_filter_mutex_;

  using VmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint32_t>>;
  std::unique_ptr<VmoStore> vmo_store_ __TA_GUARDED(vmo_lock_);
  network::SharedLock vmo_lock_;

  // Because NetworkDevice is currently DFv1 we need to have a compatibility server in place.
  std::unique_ptr<compat::SyncInitializedDeviceServer> compat_server_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
  fdf::UnsynchronizedDispatcher netdev_dispatcher_;

  // Keep these last so that they are destroyed first, canceling any pending calls to these methods
  // before they can access other data members that are destroyed.
  async::IrqMethod<Device, &Device::HandleIrq> irq_handler_{this};
  async::TaskMethod<Device<RxDescriptor>, &Device<RxDescriptor>::OnLinkStateChange>
      on_link_state_change_task_{this};
};

}  // namespace e1000

#endif  // ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_FUCHSIA_H_
