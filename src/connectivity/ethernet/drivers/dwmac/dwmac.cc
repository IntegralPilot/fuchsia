// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dwmac.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <fuchsia/hardware/ethernet/mac/c/banjo.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/hw/arch_ops.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fit/defer.h>
#include <lib/fzl/vmar-manager.h>
#include <lib/operation/ethernet.h>
#include <lib/zircon-internal/align.h>
#include <lib/zx/clock.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <zircon/compiler.h>

#include <ddktl/metadata_server.h>
#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

#include "dw-gmac-dma.h"

namespace eth {

namespace {

// MMIO Indexes.
constexpr uint32_t kEthMacMmio = 0;

constexpr char kPdevFragment[] = "pdev";

// Report an MTU of 1514. That's 1500 for the IP layer, plus 14 for the Ethernet
// header.
constexpr uint32_t kReportedMtu = 1514;

}  // namespace

template <typename T, typename U>
static inline T* offset_ptr(U* ptr, size_t offset) {
  return reinterpret_cast<T*>(reinterpret_cast<uintptr_t>(ptr) + offset);
}

int DWMacDevice::Thread() {
  zxlogf(INFO, "dwmac: ethmac started");

  zx_status_t status;

  while (true) {
    status = dma_irq_.wait(nullptr);
    if (!running_.load()) {
      status = ZX_OK;
      break;
    }
    if (status != ZX_OK) {
      zxlogf(ERROR, "dwmac: Interrupt error");
      break;
    }
    uint32_t stat = 0;
    {
      network::SharedAutoLock lock(&state_lock_);  // Note: limited scope of autolock
      if (!started_) {
        // Spurious IRQ.
        continue;
      }
      stat = mmio_->Read32(DW_MAC_DMA_STATUS);
      mmio_->Write32(stat, DW_MAC_DMA_STATUS);

      if (stat & DMA_STATUS_GLPII) {
        // Read the LPI status to clear the GLPII bit and prevent re-interrupting.
        (void)mmio_->Read32(DW_MAC_MAC_LPICONTROL);
      }

      if (stat & DMA_STATUS_RI) {
        ProcRxBuffer(stat);
      }
      if (stat & DMA_STATUS_TI) {
        ProcTxBuffer();
      }
    }
    if (stat & DMA_STATUS_GLI) {
      fbl::AutoLock lock(&state_lock_);  // Note: limited scope of autolock
      UpdateLinkStatus();
    }
    if (stat & DMA_STATUS_AIS) {
      bus_errors_.fetch_add(1, std::memory_order_relaxed);
      zxlogf(ERROR, "dwmac: abnormal interrupt. status = 0x%08x", stat);
    }
  }
  return status;
}

int DWMacDevice::WorkerThread() {
  // Note: Need to wait here for all PHY's to register
  //       their callbacks before proceeding further.
  //       Currently only supporting single PHY, we can add
  //       support for multiple PHY's easily when needed.
  sync_completion_wait(&cb_registered_signal_, ZX_TIME_INFINITE);

  // Configure the phy.
  {
    fbl::AutoLock lock(&state_lock_);
    cbs_.config_phy(cbs_.ctx, mac_.data());
  }

  auto thunk = [](void* arg) -> int { return reinterpret_cast<DWMacDevice*>(arg)->Thread(); };

  running_.store(true);
  int ret = thrd_create_with_name(&thread_, thunk, this, "mac-thread");
  ZX_DEBUG_ASSERT(ret == thrd_success);

  fbl::AllocChecker ac;
  std::unique_ptr<NetworkFunction> network_function(new (&ac) NetworkFunction(zxdev(), this));
  if (!ac.check()) {
    DdkAsyncRemove();
    return ZX_ERR_NO_MEMORY;
  }

  if (!outgoing_dir_.has_value()) {
    outgoing_dir_.emplace(fdf::OutgoingDirectory::Create(fdf::Dispatcher::GetCurrent()->get()));
  }

  auto status = network_function->Add("Designware-MAC", netdev_dispatcher_.get(), *outgoing_dir_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "dwmac: Could not create eth device: %d", status);
    DdkAsyncRemove();
    return status;
  } else {
    zxlogf(INFO, "dwmac: Added dwMac device");
  }
  network_function_ = network_function.release();

  return ZX_OK;
}

void DWMacDevice::SendPortStatus() {
  if (!netdevice_.is_valid()) {
    zxlogf(WARNING, "dwmac: System not ready");
  }
  fdf::Arena arena(0u);
  auto port_status = fuchsia_hardware_network::wire::PortStatus::Builder(arena)
                         .flags(online_ ? fuchsia_hardware_network::wire::StatusFlags::kOnline
                                        : fuchsia_hardware_network::wire::StatusFlags{0})
                         .mtu(kReportedMtu)
                         .Build();

  zxlogf(ERROR, "Communicating port status of %d", (int)online_);
  if (auto result = netdevice_.buffer(arena)->PortStatusChanged(kPortId, port_status);
      !result.ok()) {
    zxlogf(ERROR, "Failed to send port status changed: %s", result.FormatDescription().c_str());
    return;
  }
}

void DWMacDevice::UpdateLinkStatus() {
  bool temp = mmio_->ReadMasked32(GMAC_RGMII_STATUS_LNKSTS, DW_MAC_MAC_RGMIISTATUS);

  if (temp != online_) {
    online_ = temp;
    SendPortStatus();
  }
  if (online_) {
    mmio_->SetBits32((GMAC_CONF_TE | GMAC_CONF_RE), DW_MAC_MAC_CONF);
  } else {
    mmio_->ClearBits32((GMAC_CONF_TE | GMAC_CONF_RE), DW_MAC_MAC_CONF);
  }
  zxlogf(INFO, "dwmac: Link is now %s", online_ ? "up" : "down");
}

zx_status_t DWMacDevice::InitPdev() {
  // Map mac control registers and dma control registers.
  zx::result mmio = pdev_.MapMmio(kEthMacMmio);
  if (mmio.is_error()) {
    zxlogf(ERROR, "Failed to map mmio: %s", mmio.status_string());
    return mmio.status_value();
  }
  mmio_ = std::move(mmio.value());

  // Map dma interrupt.
  zx::result interrupt = pdev_.GetInterrupt(0);
  if (interrupt.is_error()) {
    zxlogf(ERROR, "Failed to get interrupt: %s", interrupt.status_string());
    return interrupt.status_value();
  }
  dma_irq_ = std::move(interrupt.value());

  // Get ETH_BOARD protocol.
  if (!eth_board_.is_valid()) {
    zxlogf(ERROR, "dwmac: could not obtain ETH_BOARD protocol");
    return ZX_ERR_BAD_STATE;
  }

  return ZX_OK;
}

zx_status_t DWMacDevice::Create(void* ctx, zx_device_t* device) {
  fdf::PDev pdev;
  {
    zx::result pdev_client = ddk::Device<void>::DdkConnectFragmentFidlProtocol<
        fuchsia_hardware_platform_device::Service::Device>(device, "pdev");
    if (pdev_client.is_error()) {
      zxlogf(ERROR, "Failed to connect to platform device: %s", pdev_client.status_string());
      return pdev_client.status_value();
    }
    pdev = fdf::PDev{std::move(pdev_client.value())};
  }

  // Get our bti.
  zx::result bti = pdev.GetBti(0);
  if (bti.is_error()) {
    zxlogf(ERROR, "Failed to get bti: %s", bti.status_string());
    return bti.status_value();
  }

  zx::result client =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_ethernet_board::Service::Device>(device,
                                                                                       "eth-board");
  if (client.is_error()) {
    zxlogf(ERROR, " failed to connect to FIDL fragment: %s",
           zx_status_get_string(client.error_value()));
    return client.error_value();
  }
  auto mac_device = std::make_unique<DWMacDevice>(device, std::move(pdev), std::move(bti.value()),
                                                  std::move(*client));

  zx_status_t status = mac_device->InitPdev();
  if (status != ZX_OK) {
    return status;
  }

  {
    fbl::AutoLock lock(&mac_device->state_lock_);
    if (zx_status_t status =
            mac_device->vmo_store_.Reserve(fuchsia_hardware_network::wire::kMaxDataVmos);
        status != ZX_OK) {
      zxlogf(ERROR, "failed to initialize vmo store: %s", zx_status_get_string(status));
      return status;
    }
  }

  // Reset the phy.
  fidl::WireResult result = mac_device->eth_board_->ResetPhy();
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to reset Phy");
    return result.status();
  }

  // Get and cache the mac address.
  mac_device->GetMAC(device);

  // Reset the dma peripheral.
  mac_device->mmio_->SetBits32(DMAMAC_SRST, DW_MAC_DMA_BUSMODE);
  uint32_t loop_count = 100;
  do {
    zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));
    loop_count--;
  } while (mac_device->mmio_->ReadMasked32(DMAMAC_SRST, DW_MAC_DMA_BUSMODE) && loop_count);
  if (!loop_count) {
    zxlogf(ERROR, "Timed out while attempting to read bus mode");
    return ZX_ERR_TIMED_OUT;
  }

  // Mac address register was erased by the reset; set it!
  {
    fbl::AutoLock lock(&mac_device->state_lock_);
    mac_device->mmio_->Write32((mac_device->mac_[5] << 8) | (mac_device->mac_[4] << 0),
                               DW_MAC_MAC_MACADDR0HI);
    mac_device->mmio_->Write32((mac_device->mac_[3] << 24) | (mac_device->mac_[2] << 16) |
                                   (mac_device->mac_[1] << 8) | (mac_device->mac_[0] << 0),
                               DW_MAC_MAC_MACADDR0LO);
  }

  auto cleanup = fit::defer([&]() { mac_device->ShutDown(); });

  // Don't create the dispatchers before the deferred cleanup task has been created. We want to
  // ensure that they are properly shutdown on any failures after this. If they are created before
  // the cleanup task the dispatcher objects might be destroyed before the dispatchers are shut
  // down, which triggers an assert.
  zx::result netdev_dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "netdev", [device = mac_device.get()](fdf_dispatcher_t*) {
        device->netdev_dispatcher_shutdown_.Signal();
      });
  if (netdev_dispatcher.is_error()) {
    zxlogf(ERROR, "Failed to create netdevice dispatcher: %s", netdev_dispatcher.status_string());
    return netdev_dispatcher.error_value();
  }
  mac_device->netdev_dispatcher_ = std::move(netdev_dispatcher.value());

  zx::result outgoing_dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "outgoing", [device = mac_device.get()](fdf_dispatcher_t*) {
        device->outgoing_dispatcher_shutdown_.Signal();
      });
  if (outgoing_dispatcher.is_error()) {
    zxlogf(ERROR, "Failed to create outgoing dispatcher: %s", outgoing_dispatcher.status_string());
    return outgoing_dispatcher.error_value();
  }
  mac_device->outgoing_dispatcher_ = std::move(outgoing_dispatcher.value());

  status = mac_device->AllocateBuffers();
  if (status != ZX_OK) {
    return status;
  }

  status = mac_device->InitBuffers();
  if (status != ZX_OK)
    return status;

  sync_completion_reset(&mac_device->cb_registered_signal_);

  status = mac_device->DdkAdd("dwmac", DEVICE_ADD_NON_BINDABLE);
  if (status != ZX_OK) {
    zxlogf(ERROR, "DWMac DdkAdd failed %d", status);
    return status;
  }

  // mac_device intentionally leaked as it is now held by DevMgr. Make sure this happens immediately
  // after DdkAdd. Otherwise we're holding on to a unique pointer to something we don't own. If
  // subsequent calls fail we would then destroy it twice. Once from the unique pointer destructor
  // and once from DdkRelease.
  DWMacDevice* unowned_device = mac_device.release();
  // NOTE: After this it's not safe to use mac_device, it is now a null pointer. Use unowned_device.

  // At this point we must also cancel the cleanup handler. Now that the DDK owns the pointer it
  // will call DdkUnbind if Create returns an error. DdkUnbind will then clean things up.
  cleanup.cancel();

  fbl::AllocChecker ac;
  std::unique_ptr<EthMacFunction> mac_function(
      new (&ac) EthMacFunction(unowned_device->zxdev(), unowned_device));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  // TODO(braval): use proper device pointer, depending on how
  //               many PHY devices we have to load, from the metadata.
  status = mac_function->DdkAdd(ddk::DeviceAddArgs("eth_phy"));
  if (status != ZX_OK) {
    zxlogf(ERROR, "DdkAdd for Eth Mac Function failed %d", status);
    return status;
  }
  unowned_device->mac_function_ = mac_function.release();

  // This will wait for hardware callbacks to be registered and then add the network device child
  // node along with the network device service in the outgoing directory. All access to the
  // outgoing directory needs to happen on the same dispatcher. Ensure this task runs on the
  // dispatcher designated for this purpose.
  async::PostTask(unowned_device->outgoing_dispatcher_.async_dispatcher(),
                  [unowned_device] { unowned_device->WorkerThread(); });

  return ZX_OK;
}  // namespace eth

zx_status_t DWMacDevice::AllocateBuffers() {
  std::lock_guard rx_lock(rx_lock_);
  std::lock_guard tx_lock(tx_lock_);

  const size_t desc_size =
      ZX_ROUNDUP(2ul * kNumDesc * sizeof(dw_dmadescr_t), zx_system_get_page_size());
  desc_buffer_ = PinnedBuffer::Create(desc_size, bti_, ZX_CACHE_POLICY_UNCACHED);

  tx_descriptors_ = static_cast<dw_dmadescr_t*>(desc_buffer_->GetBaseAddress());
  // rx descriptors right after tx
  rx_descriptors_ = &tx_descriptors_[kNumDesc];

  return ZX_OK;
}

zx_status_t DWMacDevice::InitBuffers() {
  std::lock_guard rx_lock(rx_lock_);
  std::lock_guard tx_lock(tx_lock_);

  zx_paddr_t tmpaddr;

  // Initialize descriptors. Doing tx and rx all at once
  for (uint i = 0; i < kNumDesc; i++) {
    desc_buffer_->LookupPhys(((i + 1) % kNumDesc) * sizeof(dw_dmadescr_t), &tmpaddr);
    tx_descriptors_[i].dmamac_next = static_cast<uint32_t>(tmpaddr);

    tx_descriptors_[i].txrx_status = 0;
    tx_descriptors_[i].dmamac_cntl = DESC_TXCTRL_TXCHAIN;

    desc_buffer_->LookupPhys((((i + 1) % kNumDesc) + kNumDesc) * sizeof(dw_dmadescr_t), &tmpaddr);
    rx_descriptors_[i].dmamac_next = static_cast<uint32_t>(tmpaddr);

    rx_descriptors_[i].dmamac_cntl =
        (MAC_MAX_FRAME_SZ & DESC_RXCTRL_SIZE1MASK) | DESC_RXCTRL_RXCHAIN;

    rx_descriptors_[i].txrx_status = 0;
  }

  tx_head_ = 0;
  tx_tail_ = 0;
  rx_head_ = 0;
  rx_tail_ = 0;
  rx_queued_ = 0;

  hw_wmb();

  desc_buffer_->LookupPhys(0, &tmpaddr);
  mmio_->Write32(static_cast<uint32_t>(tmpaddr), DW_MAC_DMA_TXDESCLISTADDR);

  desc_buffer_->LookupPhys(kNumDesc * sizeof(dw_dmadescr_t), &tmpaddr);
  mmio_->Write32(static_cast<uint32_t>(tmpaddr), DW_MAC_DMA_RXDESCLISTADDR);

  return ZX_OK;
}

zx_status_t DWMacDevice::EthMacMdioWrite(uint32_t reg, uint32_t val) {
  mmio_->Write32(val, DW_MAC_MAC_MIIDATA);

  uint32_t miiaddr = (mii_addr_ << MIIADDRSHIFT) | (reg << MIIREGSHIFT) | MII_WRITE;

  mmio_->Write32(miiaddr | MII_CLKRANGE_150_250M | MII_BUSY, DW_MAC_MAC_MIIADDR);

  zx::time deadline = zx::deadline_after(zx::msec(3));
  do {
    if (!mmio_->ReadMasked32(MII_BUSY, DW_MAC_MAC_MIIADDR)) {
      return ZX_OK;
    }
    zx::nanosleep(zx::deadline_after(zx::usec(10)));
  } while (zx::clock::get_monotonic() < deadline);
  return ZX_ERR_TIMED_OUT;
}

zx_status_t DWMacDevice::EthMacMdioRead(uint32_t reg, uint32_t* val) {
  uint32_t miiaddr = (mii_addr_ << MIIADDRSHIFT) | (reg << MIIREGSHIFT);

  mmio_->Write32(miiaddr | MII_CLKRANGE_150_250M | MII_BUSY, DW_MAC_MAC_MIIADDR);

  zx::time deadline = zx::deadline_after(zx::msec(3));
  do {
    if (!mmio_->ReadMasked32(MII_BUSY, DW_MAC_MAC_MIIADDR)) {
      *val = mmio_->Read32(DW_MAC_MAC_MIIDATA);
      return ZX_OK;
    }
    zx::nanosleep(zx::deadline_after(zx::usec(10)));
  } while (zx::clock::get_monotonic() < deadline);
  return ZX_ERR_TIMED_OUT;
}

zx_status_t DWMacDevice::EthMacRegisterCallbacks(const eth_mac_callbacks_t* cbs) {
  if (cbs == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }

  cbs_ = *cbs;

  sync_completion_signal(&cb_registered_signal_);
  return ZX_OK;
}

DWMacDevice::DWMacDevice(zx_device_t* device, fdf::PDev pdev, zx::bti bti,
                         fidl::ClientEnd<fuchsia_hardware_ethernet_board::EthBoard> eth_board)
    : ddk::Device<DWMacDevice, ddk::Unbindable, ddk::Suspendable>(device),
      bti_(std::move(bti)),
      pdev_(std::move(pdev)),
      eth_board_(fidl::WireSyncClient(std::move(eth_board))),
      vmo_store_({
          .map =
              vmo_store::MapOptions{
                  .vm_option = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
                  .vmar = nullptr,
              },
          .pin =
              vmo_store::PinOptions{
                  .bti = zx::unowned_bti(bti_),
                  .bti_pin_options = ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE,
                  .index = true,
              },
      }) {}

void DWMacDevice::ReleaseBuffers() {
  // Unpin the memory used for the dma buffers
  if (desc_buffer_->UnPin() != ZX_OK) {
    zxlogf(ERROR, "dwmac: Error unpinning description buffers");
  }
}

void DWMacDevice::DdkRelease() {
  zxlogf(INFO, "Ethernet release...");
  delete this;
}

void DWMacDevice::DdkUnbind(ddk::UnbindTxn txn) {
  zxlogf(INFO, "Ethernet DdkUnbind");
  ShutDown();
  txn.Reply();
}

void DWMacDevice::DdkSuspend(ddk::SuspendTxn txn) {
  zxlogf(INFO, "Ethernet DdkSuspend");
  // We do not distinguish between states, just completely shutdown.
  ShutDown();
  txn.Reply(ZX_OK, txn.requested_state());
}

zx_status_t DWMacDevice::ShutDown() {
  if (running_.load()) {
    running_.store(false);
    dma_irq_.destroy();
    thrd_join(thread_, NULL);
  }
  {
    fbl::AutoLock lock(&state_lock_);
    online_ = false;
    netdevice_ = {};
    DeInitDevice();
    ReleaseBuffers();
  }
  if (netdev_dispatcher_.get()) {
    netdev_dispatcher_.ShutdownAsync();
    netdev_dispatcher_shutdown_.Wait();
  }
  if (outgoing_dispatcher_.get()) {
    if (outgoing_dir_.has_value()) {
      libsync::Completion outgoing_destroyed;
      async::PostTask(outgoing_dispatcher_.async_dispatcher(), [&] {
        outgoing_dir_.reset();
        outgoing_destroyed.Signal();
      });
      outgoing_destroyed.Wait();
    }
    outgoing_dispatcher_.ShutdownAsync();
    outgoing_dispatcher_shutdown_.Wait();
  }
  return ZX_OK;
}

zx_status_t DWMacDevice::GetMAC(zx_device_t* dev) {
  // look for MAC address device metadata
  zx::result metadata_result =
      ddk::GetMetadataIfExists<fuchsia_boot_metadata::MacAddressMetadata>(dev, kPdevFragment);
  if (metadata_result.is_error()) {
    zxlogf(ERROR, "Failed to get MAC address metadata: %s", metadata_result.status_string());
    return metadata_result.status_value();
  }

  std::array<uint8_t, 6> octets;
  if (metadata_result.value().has_value()) {
    const auto& metadata = metadata_result.value().value();
    if (!metadata.mac_address().has_value()) {
      zxlogf(ERROR, "MAC address metadata missing mac_address field");
      return ZX_ERR_INTERNAL;
    }
    octets = metadata.mac_address().value().octets();
  } else {
    zxlogf(WARNING, "Falling back on HW setting: MAC address metadata does not exist");

    // read MAC address from hardware register
    uint32_t hi = mmio_->Read32(DW_MAC_MAC_MACADDR0HI);
    uint32_t lo = mmio_->Read32(DW_MAC_MAC_MACADDR0LO);

    /* Extract the MAC address from the high and low words */
    octets[0] = static_cast<uint8_t>(lo & 0xff);
    octets[1] = static_cast<uint8_t>((lo >> 8) & 0xff);
    octets[2] = static_cast<uint8_t>((lo >> 16) & 0xff);
    octets[3] = static_cast<uint8_t>((lo >> 24) & 0xff);
    octets[4] = static_cast<uint8_t>(hi & 0xff);
    octets[5] = static_cast<uint8_t>((hi >> 8) & 0xff);
  }

  zxlogf(INFO, "MAC address %02x:%02x:%02x:%02x:%02x:%02x", octets[0], octets[1], octets[2],
         octets[3], octets[4], octets[5]);
  fbl::AutoLock lock(&state_lock_);
  mac_ = octets;
  return ZX_OK;
}

void DWMacDevice::NetworkDeviceImplInit(
    netdev::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::InitCompleter::Sync& completer) {
  fbl::AutoLock lock(&state_lock_);
  if (netdevice_.is_valid()) {
    completer.buffer(arena).Reply(ZX_ERR_ALREADY_BOUND);
    return;
  }
  netdevice_.Bind(std::move(request->iface), netdev_dispatcher_.get());

  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::NetworkPort>::Create();
  fdf::BindServer(netdev_dispatcher_.get(), std::move(server), network_function_);

  netdevice_.buffer(arena)
      ->AddPort(kPortId, std::move(client))
      .Then([completer = completer.ToAsync()](
                fdf::WireUnownedResult<netdev::NetworkDeviceIfc::AddPort>& result) mutable {
        fdf::Arena arena(0u);
        if (!result.ok()) {
          zxlogf(ERROR, "Failed to add port: %s", result.FormatDescription().c_str());
          completer.buffer(arena).Reply(result.status());
          return;
        }
        if (result->status != ZX_OK) {
          zxlogf(ERROR, "Failed to add port: %s", zx_status_get_string(result->status));
          completer.buffer(arena).Reply(result->status);
          return;
        }
        completer.buffer(arena).Reply(ZX_OK);
      });
}

zx_status_t DWMacDevice::InitDevice() {
  mmio_->Write32(0, DW_MAC_DMA_INTENABLE);

  mmio_->Write32(X8PBL | DMA_PBL, DW_MAC_DMA_BUSMODE);

  mmio_->Write32(DMA_OPMODE_TSF | DMA_OPMODE_RSF, DW_MAC_DMA_OPMODE);
  mmio_->SetBits32(DMA_OPMODE_SR | DMA_OPMODE_ST, DW_MAC_DMA_OPMODE);

  // Clear all the interrupt flags
  mmio_->Write32(~0, DW_MAC_DMA_STATUS);

  // Disable(mask) interrupts generated by the mmc block
  mmio_->Write32(~0, DW_MAC_MMC_INTR_MASK_RX);
  mmio_->Write32(~0, DW_MAC_MMC_INTR_MASK_TX);
  mmio_->Write32(~0, DW_MAC_MMC_IPC_INTR_MASK_RX);

  // Enable Interrupts
  mmio_->Write32(DMA_INT_NIE | DMA_INT_AIE | DMA_INT_FBE | DMA_INT_RIE | DMA_INT_RUE | DMA_INT_OVE |
                     DMA_INT_UNE | DMA_INT_TSE | DMA_INT_RSE | DMA_INT_TIE,
                 DW_MAC_DMA_INTENABLE);

  mmio_->Write32(0, DW_MAC_MAC_MACADDR0HI);
  mmio_->Write32(0, DW_MAC_MAC_MACADDR0LO);
  mmio_->Write32(~0, DW_MAC_MAC_HASHTABLEHI);
  mmio_->Write32(~0, DW_MAC_MAC_HASHTABLELO);

  // TODO - configure filters
  zxlogf(INFO, "macaddr0hi = %08x", mmio_->Read32(DW_MAC_MAC_MACADDR0HI));
  zxlogf(INFO, "macaddr0lo = %08x", mmio_->Read32(DW_MAC_MAC_MACADDR0LO));

  mmio_->SetBits32((1 << 10) | (1 << 4) | (1 << 0), DW_MAC_MAC_FRAMEFILT);

  mmio_->Write32(GMAC_CORE_INIT, DW_MAC_MAC_CONF);

  return ZX_OK;
}

zx_status_t DWMacDevice::DeInitDevice() {
  // Disable Interrupts
  mmio_->Write32(0, DW_MAC_DMA_INTENABLE);

  // Disable Transmit and Receive
  mmio_->ClearBits32(GMAC_CONF_TE | GMAC_CONF_RE, DW_MAC_MAC_CONF);

  // reset the phy (hold in reset)
  // gpio_write(&gpios_[PHY_RESET], 0);

  // transmit and receive are now disabled, safe to null descriptor list ptrs
  mmio_->Write32(0, DW_MAC_DMA_TXDESCLISTADDR);
  mmio_->Write32(0, DW_MAC_DMA_RXDESCLISTADDR);

  return ZX_OK;
}

uint32_t DWMacDevice::DmaRxStatus() {
  return mmio_->ReadMasked32(DMA_STATUS_RS_MASK, DW_MAC_DMA_STATUS) >> DMA_STATUS_RS_POS;
}

void DWMacDevice::ProcRxBuffer(uint32_t int_status) {
  if (!started_) {
    return;
  }

  fdf::Arena arena(0u);

  std::lock_guard lock(rx_lock_);

  // Batch completions.
  constexpr size_t kBatchRxBufs = 64;
  __UNINITIALIZED netdev::wire::RxBufferPart rx_buffer_parts[kBatchRxBufs];
  __UNINITIALIZED netdev::wire::RxBuffer rx_buffer[kBatchRxBufs];
  size_t num_rx_completed = 0;
  while (rx_queued_ > 0) {
    uint32_t pkt_stat = rx_descriptors_[rx_tail_].txrx_status;

    if (pkt_stat & DESC_RXSTS_OWNBYDMA) {
      break;
    }
    size_t fr_len = (pkt_stat & DESC_RXSTS_FRMLENMSK) >> DESC_RXSTS_FRMLENSHFT;
    if (fr_len > kMtu) {
      zxlogf(ERROR, "dwmac: unsupported packet size received");
      return;
    }

    auto [id, addr] = rx_in_flight_buffer_ids_[rx_tail_];

    zx_cache_flush(addr, kMtu, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);

    rx_buffer_parts[num_rx_completed] = netdev::wire::RxBufferPart{
        .id = id,
        .offset = 0,
        .length = static_cast<uint32_t>(fr_len),
    };
    rx_buffer[num_rx_completed] = netdev::wire::RxBuffer{
        .meta = {.port = kPortId,
                 .flags = 0,
                 .frame_type = fuchsia_hardware_network::wire::FrameType::kEthernet},
        .data = fidl::VectorView<netdev::wire::RxBufferPart>::FromExternal(
            &rx_buffer_parts[num_rx_completed], 1),
    };

    rx_packet_.fetch_add(1, std::memory_order_relaxed);

    rx_tail_ = (rx_tail_ + 1) % kNumDesc;
    if (rx_tail_ == 0) {
      loop_count_.fetch_add(1, std::memory_order_relaxed);
    }
    num_rx_completed++;
    rx_queued_--;
    if (num_rx_completed == kBatchRxBufs) {
      fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteRx(
          fidl::VectorView<netdev::wire::RxBuffer>::FromExternal(rx_buffer, num_rx_completed));
      num_rx_completed = 0;
      if (!status.ok()) {
        zxlogf(ERROR, "Failed to complete RX: %s", status.FormatDescription().c_str());
        // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
        DdkAsyncRemove();
        return;
      }
    }
  }
  if (num_rx_completed != 0) {
    fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteRx(
        fidl::VectorView<netdev::wire::RxBuffer>::FromExternal(rx_buffer, num_rx_completed));
    if (!status.ok()) {
      zxlogf(ERROR, "Failed to complete RX: %s", status.FormatDescription().c_str());
      // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
      DdkAsyncRemove();
    }
  }
}

void DWMacDevice::ProcTxBuffer() {
  if (!started_) {
    return;
  }
  ZX_DEBUG_ASSERT(netdevice_.is_valid());

  fdf::Arena arena(0u);

  std::lock_guard lock(tx_lock_);

  size_t tx_complete = 0;
  uint32_t tx_complete_start = tx_tail_;

  while (true) {
    if (!tx_in_flight_active_[tx_tail_]) {
      break;
    }
    uint32_t pkt_stat = tx_descriptors_[tx_tail_].txrx_status;

    if (pkt_stat & DESC_TXSTS_OWNBYDMA) {
      break;
    }

    tx_in_flight_active_[tx_tail_] = false;
    tx_in_flight_buffer_ids_[tx_tail_].status = ZX_OK;
    tx_complete++;

    tx_tail_ = (tx_tail_ + 1) % kNumDesc;
    if (tx_tail_ == 0) {
      fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteTx(
          fidl::VectorView<netdev::wire::TxResult>::FromExternal(
              &tx_in_flight_buffer_ids_[tx_complete_start], tx_complete));
      tx_complete_start = 0;
      tx_complete = 0;
      if (!status.ok()) {
        zxlogf(ERROR, "Failed to complete TX: %s", status.FormatDescription().c_str());
        // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
        DdkAsyncRemove();
        return;
      }
    }
  }

  if (tx_complete > 0) {
    fidl::OneWayStatus status =
        netdevice_.buffer(arena)->CompleteTx(fidl::VectorView<netdev::wire::TxResult>::FromExternal(
            &tx_in_flight_buffer_ids_[tx_complete_start], tx_complete));
    if (!status.ok()) {
      zxlogf(ERROR, "Failed to complete TX: %s", status.FormatDescription().c_str());
      // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
      DdkAsyncRemove();
    }
  }
}

void DWMacDevice::NetworkDeviceImplStart(
    fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::StartCompleter::Sync& completer) {
  zx_status_t result = ZX_OK;
  {
    fbl::AutoLock lock(&state_lock_);
    if (started_) {
      zxlogf(WARNING, "device already started");
      result = ZX_ERR_ALREADY_BOUND;
    } else {
      result = InitBuffers();
      if (result == ZX_OK) {
        result = InitDevice();
      }
      if (result == ZX_OK) {
        started_ = true;
      }
    }
    SendPortStatus();
  }
  completer.buffer(arena).Reply(result);
}

void DWMacDevice::NetworkDeviceImplStop(
    fdf::Arena& arena, fdf::WireServer<netdev::NetworkDeviceImpl>::StopCompleter::Sync& completer) {
  {
    fbl::AutoLock lock(&state_lock_);
    // Disable TX and RX.
    DeInitDevice();
    hw_mb();
    // Can now return any buffers.
    {
      std::lock_guard tx_lock{tx_lock_};
      while (tx_in_flight_active_[tx_tail_]) {
        tx_in_flight_buffer_ids_[tx_tail_].status = ZX_ERR_UNAVAILABLE;
        fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteTx(
            fidl::VectorView<netdev::wire::TxResult>::FromExternal(
                &tx_in_flight_buffer_ids_[tx_tail_], 1));
        if (!status.ok()) {
          zxlogf(ERROR, "Failed to return TX buffers on stop: %s",
                 status.FormatDescription().c_str());
          // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
          DdkAsyncRemove();
          return;
        }
        tx_in_flight_active_[tx_tail_] = false;
        tx_tail_ = (tx_tail_ + 1) % kNumDesc;
      }
    }
    {
      std::lock_guard rx_lock{rx_lock_};
      while (rx_queued_ > 0) {
        auto [id, addr] = rx_in_flight_buffer_ids_[rx_tail_];
        netdev::wire::RxBufferPart part{
            .id = id,
            .length = 0,
        };
        netdev::wire::RxBuffer buf = {
            .meta = {.frame_type = fuchsia_hardware_network::FrameType::kEthernet},
            .data = fidl::VectorView<netdev::wire::RxBufferPart>::FromExternal(&part, 1),
        };
        fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteRx(
            fidl::VectorView<netdev::wire::RxBuffer>::FromExternal(&buf, 1));
        if (!status.ok()) {
          zxlogf(ERROR, "Failed to return RX buffers on stop: %s",
                 status.FormatDescription().c_str());
          // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
          DdkAsyncRemove();
          return;
        }
        rx_tail_ = (rx_tail_ + 1) % kNumDesc;
        rx_queued_--;
      }
    }
    started_ = false;
  }
  completer.buffer(arena).Reply();
}

void DWMacDevice::NetworkDeviceImplGetInfo(
    fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) {
  netdev::wire::DeviceImplInfo info =
      netdev::wire::DeviceImplInfo::Builder(arena)
          .tx_depth(kNumDesc)
          .rx_depth(kNumDesc)
          .rx_threshold(kNumDesc / 2)
          // Ensures clients do not use scatter-gather.
          .max_buffer_parts(1)
          // Per fuchsia.hardware.network.driver API:
          // "Devices that do not support scatter-gather DMA may set this to a value smaller than
          // a page size to guarantee compatibility."
          .max_buffer_length(2048)
          // 2k alignment ensures, given our 1500 mtu, that every packet is on its own page.
          .buffer_alignment(2048)
          // Ensures that an rx buffer will always be large enough to the ethernet MTU.
          .min_rx_buffer_length(kMtu)
          .Build();

  completer.buffer(arena).Reply(info);
}

void DWMacDevice::NetworkDeviceImplQueueTx(
    netdev::wire::NetworkDeviceImplQueueTxRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::QueueTxCompleter::Sync& completer) {
  network::SharedAutoLock state_lock(&state_lock_);

  auto return_buffers = [&](size_t first_buffer,
                            zx_status_t code) __TA_REQUIRES_SHARED(state_lock_) {
    for (size_t i = first_buffer; i < request->buffers.size(); i++) {
      netdev::wire::TxResult ret{.id = request->buffers[i].id, .status = code};

      fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteTx(
          fidl::VectorView<netdev::wire::TxResult>::FromExternal(&ret, 1));
      if (!status.ok()) {
        zxlogf(ERROR, "Failed to complete TX: %s", status.FormatDescription().c_str());
        // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
        // Note that this may result in multiple calls to DdkAsyncRemove below but according to the
        // DDK this is safe and the additional calls have no effect.
        DdkAsyncRemove();
      }
    }
  };

  if (!started_) {
    zxlogf(ERROR, "tx buffers queued before start call");
    return_buffers(0, ZX_ERR_UNAVAILABLE);
    return;
  }

  std::lock_guard lock{tx_lock_};

  for (size_t i = 0; i < request->buffers.size(); i++) {
    const netdev::wire::TxBuffer& buffer = request->buffers[i];

    if (buffer.data.size() != 1) {
      zxlogf(ERROR, "received unsupported scatter gather buffer");
      return_buffers(i, ZX_ERR_INVALID_ARGS);
      return;
    }
    const netdev::wire::BufferRegion& data = buffer.data[0];

    if (data.length > kMtu) {
      zxlogf(ERROR, "tx buffer length is too large");
      return_buffers(i, ZX_ERR_INVALID_ARGS);
      DdkAsyncRemove();
      return;
    }

    if (tx_in_flight_active_[tx_head_]) {
      zxlogf(ERROR, "tx buffers exceeded published depth");
      return_buffers(i, ZX_ERR_INTERNAL);
      DdkAsyncRemove();
      return;
    }

    ZX_DEBUG_ASSERT(!(tx_descriptors_[tx_head_].txrx_status & DESC_TXSTS_OWNBYDMA));

    VmoStore::StoredVmo* stored_vmo = vmo_store_.GetVmo(data.vmo);
    ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %d", data.vmo);

    tx_in_flight_buffer_ids_[tx_head_].id = buffer.id;
    tx_in_flight_active_[tx_head_] = true;

    // Clean the cache.
    auto mapped_span = stored_vmo->data().subspan(data.offset, data.length);
    zx_status_t status =
        zx_cache_flush(mapped_span.data(), mapped_span.size(), ZX_CACHE_FLUSH_DATA);
    ZX_DEBUG_ASSERT(status == ZX_OK);

    // Lookup the physical address. Based on our specified alignment and sizes it should be an error
    // for this to span more than one physical address.
    fzl::PinnedVmo::Region region;
    size_t actual_regions = 0;
    status = stored_vmo->GetPinnedRegions(data.offset, data.length, &region, 1, &actual_regions);
    ZX_ASSERT_MSG(status == ZX_OK && actual_regions == 1,
                  "failed to retrieve pinned region %s (actual=%zu)", zx_status_get_string(status),
                  actual_regions);

    // Check if this is the last buffer in the list, and place the TX interrupt on it if so.
    const bool last = (i + 1) == request->buffers.size();

    // Setup the length, control fields and address.
    tx_descriptors_[tx_head_].dmamac_addr = static_cast<uint32_t>(region.phys_addr);
    tx_descriptors_[tx_head_].dmamac_cntl =
        (last ? DESC_TXCTRL_TXINT : 0) | DESC_TXCTRL_TXLAST | DESC_TXCTRL_TXFIRST |
        DESC_TXCTRL_TXCHAIN | (static_cast<uint32_t>(data.length) & DESC_TXCTRL_SIZE1MASK);
    // Ensure our descriptor updates can be seen prior to the device observing that it owns the
    // buffer.
    hw_wmb();
    tx_descriptors_[tx_head_].txrx_status = DESC_TXSTS_OWNBYDMA;
    tx_head_ = (tx_head_ + 1) % kNumDesc;

    tx_counter_.fetch_add(1, std::memory_order_relaxed);
  }
  // Ensure the descriptor status is seen prior to notifying the device.
  hw_wmb();
  mmio_->Write32(~0, DW_MAC_DMA_TXPOLLDEMAND);
}

void DWMacDevice::NetworkDeviceImplQueueRxSpace(
    netdev::wire::NetworkDeviceImplQueueRxSpaceRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::QueueRxSpaceCompleter::Sync& completer) {
  network::SharedAutoLock state_lock(&state_lock_);

  auto return_buffers = [&](size_t first_buffer,
                            zx_status_t code) __TA_REQUIRES_SHARED(state_lock_) {
    for (size_t i = first_buffer; i < request->buffers.size(); i++) {
      netdev::wire::RxBufferPart part = {.id = request->buffers[i].id, .length = 0};
      netdev::wire::RxBuffer buf = {
          .meta = {.frame_type = fuchsia_hardware_network::FrameType::kEthernet},
          .data = fidl::VectorView<netdev::wire::RxBufferPart>::FromExternal(&part, 1),
      };
      fidl::OneWayStatus status = netdevice_.buffer(arena)->CompleteRx(
          fidl::VectorView<netdev::wire::RxBuffer>::FromExternal(&buf, 1));
      if (!status.ok()) {
        zxlogf(ERROR, "Failed to complete RX buffer: %s", status.FormatDescription().c_str());
        // Critical error, the driver must rebind to re-establish netdev connection and reset PHY.
        // Note that this may result in multiple calls to DdkAsyncRemove below but according to the
        // DDK this is safe and the additional calls have no effect.
        DdkAsyncRemove();
      }
    }
  };

  if (!started_) {
    zxlogf(ERROR, "rx buffers queued before start call");
    return_buffers(0, ZX_ERR_UNAVAILABLE);
    return;
  }

  std::lock_guard lock{rx_lock_};

  for (size_t i = 0; i < request->buffers.size(); i++) {
    const netdev::wire::RxSpaceBuffer& buffer = request->buffers[i];

    if (rx_queued_ == kNumDesc) {
      zxlogf(ERROR, "Too many rx buffers queued");
      return_buffers(i, ZX_ERR_INTERNAL);
      DdkAsyncRemove();
      return;
    }
    ZX_DEBUG_ASSERT(!(rx_descriptors_[rx_head_].txrx_status & DESC_RXSTS_OWNBYDMA));

    const netdev::wire::BufferRegion& data = buffer.region;

    if (data.length < kMtu) {
      zxlogf(ERROR, "rx buffer queued with length below mtu");
      return_buffers(i, ZX_ERR_INVALID_ARGS);
      DdkAsyncRemove();
      return;
    }
    // Limit the length to the MTU to ensure we do not spuriously fail the GetPinnedRegions lookup
    // later.

    VmoStore::StoredVmo* stored_vmo = vmo_store_.GetVmo(data.vmo);
    ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %d", data.vmo);

    // Clean the buffer as we do not trust the client to have done so.
    auto mapped_span = stored_vmo->data().subspan(data.offset, kMtu);
    zx_status_t status = zx_cache_flush(mapped_span.data(), mapped_span.size(),
                                        ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    ZX_DEBUG_ASSERT(status == ZX_OK);

    rx_in_flight_buffer_ids_[rx_head_] = {buffer.id, mapped_span.data()};

    // Lookup the physical address. Based on our specified alignment and sizes it should be an error
    // for this to span more than one physical address.
    fzl::PinnedVmo::Region region;
    size_t actual_regions = 0;
    status = stored_vmo->GetPinnedRegions(data.offset, kMtu, &region, 1, &actual_regions);
    ZX_ASSERT_MSG(status == ZX_OK && actual_regions == 1,
                  "failed to retrieve pinned region %s (actual=%zu)", zx_status_get_string(status),
                  actual_regions);

    rx_descriptors_[rx_head_].dmamac_addr = static_cast<uint32_t>(region.phys_addr);
    hw_wmb();
    rx_descriptors_[rx_head_].txrx_status = DESC_RXSTS_OWNBYDMA;

    rx_head_ = (rx_head_ + 1) % kNumDesc;
    rx_queued_++;
  }
  hw_wmb();
  mmio_->Write32(~0, DW_MAC_DMA_RXPOLLDEMAND);
}

void DWMacDevice::NetworkDeviceImplPrepareVmo(
    netdev::wire::NetworkDeviceImplPrepareVmoRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::PrepareVmoCompleter::Sync& completer) {
  zx_status_t status;
  {
    fbl::AutoLock lock(&state_lock_);
    status = vmo_store_.RegisterWithKey(request->id, std::move(request->vmo));
  }
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to register vmo id = %d: %s", request->id, zx_status_get_string(status));
  }
  completer.buffer(arena).Reply(status);
}

void DWMacDevice::NetworkDeviceImplReleaseVmo(
    netdev::wire::NetworkDeviceImplReleaseVmoRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::ReleaseVmoCompleter::Sync& completer) {
  fbl::AutoLock lock(&state_lock_);
  if (zx::result<zx::vmo> status = vmo_store_.Unregister(request->id);
      status.status_value() != ZX_OK) {
    // A failure here may be the result of a failed call to register the vmo, in which case the
    // driver is queued for removal from device manager. Accordingly, we must not panic lest we
    // disrupt the orderly shutdown of the driver: a log statement is the best we can do.
    zxlogf(ERROR, "failed to release vmo id = %d: %s", request->id, status.status_string());
  }
  completer.buffer(arena).Reply();
}

void DWMacDevice::NetworkPortGetInfo(
    fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::GetInfoCompleter::Sync& completer) {
  static constexpr fuchsia_hardware_network::wire::FrameType kRxTypesList[] = {
      fuchsia_hardware_network::wire::FrameType::kEthernet};
  static constexpr fuchsia_hardware_network::wire::FrameTypeSupport kTxTypesList[] = {{
      .type = fuchsia_hardware_network::wire::FrameType::kEthernet,
      .features = fuchsia_hardware_network::wire::kFrameFeaturesRaw,
  }};
  fuchsia_hardware_network::wire::PortBaseInfo info =
      fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena)
          .port_class(fuchsia_hardware_network::wire::PortClass::kEthernet)
          .rx_types(kRxTypesList)
          .tx_types(kTxTypesList)
          .Build();
  completer.buffer(arena).Reply(info);
}

void DWMacDevice::NetworkPortGetStatus(
    fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::GetStatusCompleter::Sync& completer) {
  network::SharedAutoLock lock{&state_lock_};

  fuchsia_hardware_network::wire::PortStatus status =
      fuchsia_hardware_network::wire::PortStatus::Builder(arena)
          .flags(online_ ? fuchsia_hardware_network::wire::StatusFlags::kOnline
                         : fuchsia_hardware_network::wire::StatusFlags{0})
          .mtu(kReportedMtu)
          .Build();
  completer.buffer(arena).Reply(status);
}

void DWMacDevice::NetworkPortSetActive(
    fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkPort>::SetActiveCompleter::Sync& completer) {}

void DWMacDevice::NetworkPortGetMac(
    fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::GetMacCompleter::Sync& completer) {
  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::MacAddr>::Create();
  fdf::BindServer(netdev_dispatcher_.get(), std::move(server), network_function_);
  completer.buffer(arena).Reply(std::move(client));
}

void DWMacDevice::NetworkPortRemoved(
    fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::RemovedCompleter::Sync& completer) {
  zxlogf(INFO, "removed event for port %d", kPortId);
}

void DWMacDevice::MacAddrGetAddress(
    fdf::Arena& arena, fdf::WireServer<netdev::MacAddr>::GetAddressCompleter::Sync& completer) {
  fuchsia_net::wire::MacAddress mac;
  static_assert(sizeof(mac_) == decltype(mac.octets)::size());
  {
    network::SharedAutoLock lock{&state_lock_};
    std::copy(mac_.begin(), mac_.end(), mac.octets.begin());
  }
  completer.buffer(arena).Reply(mac);
}

void DWMacDevice::MacAddrGetFeatures(
    fdf::Arena& arena, fdf::WireServer<netdev::MacAddr>::GetFeaturesCompleter::Sync& completer) {
  netdev::wire::Features features =
      netdev::wire::Features::Builder(arena)
          .supported_modes(netdev::wire::SupportedMacFilterMode::kPromiscuous)
          .Build();

  completer.buffer(arena).Reply(features);
}

void DWMacDevice::MacAddrSetMode(
    fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request, fdf::Arena& arena,
    fdf::WireServer<netdev::MacAddr>::SetModeCompleter::Sync& completer) {
  zxlogf(INFO, "Ignoring request to set mac mode");
  completer.buffer(arena).Reply();
}

zx_status_t NetworkFunction::Add(const char* name, fdf_dispatcher_t* dispatcher,
                                 fdf::OutgoingDirectory& outgoing_dir) {
  // This callback will be invoked when this service is being connected.
  auto protocol = [dispatcher, this](fdf::ServerEnd<netdev::NetworkDeviceImpl> server_end) mutable {
    fdf::BindServer(dispatcher, std::move(server_end), this);
  };

  // Register the callback to handler.
  netdev::Service::InstanceHandler handler({.network_device_impl = std::move(protocol)});

  auto status = outgoing_dir.AddService<netdev::Service>(std::move(handler));
  if (status.is_error()) {
    zxlogf(ERROR, "Failed to add service to outgoing directory: %s\n", status.status_string());
    return status.error_value();
  }

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  // Serve the outgoing directory to the entity that intends to open it, which
  // is DFv1 in this case.
  auto result = outgoing_dir.Serve(std::move(server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to serve outgoing directory: %s\n", result.status_string());
    return result.error_value();
  }

  fdf::Arena arena(0u);
  std::array offers{netdev::Service::Name};

  auto args = ddk::DeviceAddArgs(name)
                  .set_fidl_service_offers(offers)
                  .set_outgoing_dir(client.TakeChannel())
                  .set_proto_id(ZX_PROTOCOL_NETWORK_DEVICE_IMPL);

  if (zx_status_t status = DdkAdd(args); status != ZX_OK) {
    zxlogf(ERROR, "Failed to add network function device: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = DWMacDevice::Create;
  return ops;
}();

}  // namespace eth

ZIRCON_DRIVER(dwmac, eth::driver_ops, "designware_mac", "0.1");
