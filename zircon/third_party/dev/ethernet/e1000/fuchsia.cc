/*-
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * Copyright (c) 2016 Nicole Graziano <nicole@nextbsd.org>
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */

#include "fuchsia.h"

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/task.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/pci/constants.h>
#include <net/ethernet.h>

#include <fbl/auto_lock.h>

#include "support.h"

#define I210_LINK_DELAY 1000

/* PCI Config defines */
#define EM_BAR_TYPE(v) ((v) & EM_BAR_TYPE_MASK)
#define EM_BAR_TYPE_MASK 0x00000001
#define EM_BAR_TYPE_MMEM 0x00000000
#define EM_BAR_TYPE_IO 0x00000001
#define EM_BAR_TYPE_FLASH 0x0014
#define EM_BAR_MEM_TYPE(v) ((v) & EM_BAR_MEM_TYPE_MASK)
#define EM_BAR_MEM_TYPE_MASK 0x00000006
#define EM_BAR_MEM_TYPE_32BIT 0x00000000
#define EM_BAR_MEM_TYPE_64BIT 0x00000004
#define EM_MSIX_BAR 3 /* On 82575 */

#define IGB_RX_PTHRESH \
  ((hw->mac.type == e1000_i354) ? 12 : ((hw->mac.type <= e1000_82576) ? 16 : 8))
#define IGB_RX_HTHRESH 8
#define IGB_RX_WTHRESH ((hw->mac.type == e1000_82576) ? 1 : 4)

#define EM_RADV 64
#define EM_RDTR 0

#define MAX_INTS_PER_SEC 8000
#define DEFAULT_ITR (1000000000 / (MAX_INTS_PER_SEC * 256))

// The io-buffer lib uses DFv1 logging which needs this symbol to link.
__EXPORT
__WEAK zx_driver_rec __zircon_driver_rec__ = {
    .ops = {},
    .driver = {},
};

template <typename PtrType, typename UintType>
PtrType* PtrAdd(PtrType* ptr, UintType value) {
  return reinterpret_cast<PtrType*>(reinterpret_cast<uintptr_t>(ptr) + value);  // NOLINT
}

namespace e1000 {

namespace netdev = fuchsia_hardware_network;
namespace netdriver = fuchsia_hardware_network_driver;

// A class meant to help make calls to CompleteTx and ensure that they follow the batching limits
// imposed by the netdev protocol. Intended for creation of a temporary object each time CompleteTx
// needs to be called. When all results have been pushed to the transaction the caller must call
// Commit to finish the transaction. Note that Commit may also be called as part of Push if the
// batch limit is reached.
class CompleteTxTransaction {
 public:
  CompleteTxTransaction(struct adapter& adapter, fdf::Arena& arena)
      : adapter_(adapter), arena_(arena), tx_results_(adapter.tx_results) {}
  ~CompleteTxTransaction() {
    ZX_DEBUG_ASSERT_MSG_COND(count_ == 0, "%zu uncommitted TX results left in transaction", count_);
  }

  void Push(uint32_t id, zx_status_t status) {
    tx_results_[count_].id = id;
    tx_results_[count_].status = status;
    ++count_;
    if constexpr (kTxDepth > kTxResultsPerBatch) {
      if (count_ == kTxResultsPerBatch) {
        Commit();
      }
    }
  }

  void Commit() {
    if (count_ == 0) {
      return;
    }
    tx_results_.set_size(count_);
    if (fidl::OneWayStatus status = adapter_.ifc.buffer(arena_)->CompleteTx(tx_results_);
        !status.ok()) {
      fdf::error("Failed to complete TX: {}", status.FormatDescription());
    }
    count_ = 0;
  }

 private:
  struct adapter& adapter_;
  fdf::Arena& arena_;
  fidl::VectorView<netdriver::wire::TxResult>& tx_results_;
  size_t count_ = 0;
};

// A class meant to help make calls to CompleteRx and ensure that they follow the batching limits
// imposed by the netdev protocol. Intended for creation of a temporary object each time CompleteRx
// needs to be called. When all buffers have been pushed to the transaction the caller must call
// Commit to finish the transaction. Note that Commit may also be called as part of Push if the
// batch limit is reached.
class CompleteRxTransaction {
 public:
  CompleteRxTransaction(struct adapter& adapter, fdf::Arena& arena)
      : adapter_(adapter), arena_(arena), rx_buffers_(adapter.rx_buffers) {}
  ~CompleteRxTransaction() {
    ZX_DEBUG_ASSERT_MSG_COND(count_ == 0, "%zu uncommitted RX buffers left in transaction", count_);
  }

  void Push(uint32_t id, uint32_t length) {
    rx_buffers_[count_].data[0].id = id;
    rx_buffers_[count_].data[0].length = length;
    ++count_;
    if constexpr (kRxDepth > kRxBuffersPerBatch) {
      if (count_ == kRxBuffersPerBatch) {
        Commit();
      }
    }
  }

  void Commit() {
    if (count_ == 0) {
      return;
    }
    rx_buffers_.set_size(count_);
    if (fidl::OneWayStatus status = adapter_.ifc.buffer(arena_)->CompleteRx(rx_buffers_);
        !status.ok()) {
      fdf::error("Failed to complete RX: {}", status.FormatDescription());
    }
    count_ = 0;
  }

 private:
  struct adapter& adapter_;
  fdf::Arena& arena_;
  fidl::VectorView<netdriver::wire::RxBuffer>& rx_buffers_;
  size_t count_ = 0;
};

zx::result<> IdentifyHardware(struct e1000_pci* pci, struct e1000_hw& hw) {
  /* Make sure our PCI config space has the necessary stuff set */
  e1000_read_pci_cfg(&hw, static_cast<uint16_t>(fuchsia_hardware_pci::Config::kCommand),
                     &hw.bus.pci_cmd_word);

  /* Save off the information about this board */
  pci_device_info_t pci_info{};
  if (zx_status_t status = e1000_pci_get_device_info(pci, &pci_info); status != ZX_OK) {
    fdf::error("Could not get PCI device info: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  hw.vendor_id = pci_info.vendor_id;
  hw.device_id = pci_info.device_id;
  hw.revision_id = pci_info.revision_id;

  e1000_read_pci_cfg(&hw, static_cast<uint16_t>(fuchsia_hardware_pci::Config::kSubsystemVendorId),
                     &hw.subsystem_vendor_id);
  e1000_read_pci_cfg(&hw, static_cast<uint16_t>(fuchsia_hardware_pci::Config::kSubsystemId),
                     &hw.subsystem_device_id);

  /* Do Shared Code Init and Setup */
  if (e1000_set_mac_type(&hw)) {
    fdf::error("e1000_set_mac_type init failure");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return zx::ok();
}

Driver::Driver() : fdf::DriverBase2("e1000") {}

const adapter* Driver::Adapter() const { return device_->Adapter(); }

adapter* Driver::Adapter() { return device_->Adapter(); }

zx::result<> Driver::Start(fdf::DriverContext context) {
  if (device_) {
    fdf::error("Driver already started");
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  incoming_ = context.take_incoming();
  node_name_ = context.node_name().value_or("");

  std::unique_ptr<adapter> adapter = std::make_unique<struct adapter>();
  adapter->hw.back = &adapter->osdep;
  adapter->hw.mac.max_frame_size = kMaxFrameSize;

  auto compat_server = std::make_unique<compat::SyncInitializedDeviceServer>();

  if (zx::result result = compat_server->Initialize(incoming_, outgoing(), node_name_, name());
      result.is_error()) {
    fdf::error("Failed to initialize compatibility server: {}", result);
    return result.take_error();
  }

  zx::result pci_client_end = incoming_->Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_end.is_error()) {
    fdf::error("Failed to connect to PCI device: {}", pci_client_end);
    return pci_client_end.take_error();
  }

  if (zx_status_t status = e1000_pci_create(std::move(pci_client_end.value()), &adapter->osdep.pci);
      status != ZX_OK) {
    fdf::error("Failed to create e1000 PCI struct: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  struct e1000_pci* pci = adapter->osdep.pci;

  if (zx_status_t status = e1000_pci_set_bus_mastering(pci, true); status != ZX_OK) {
    fdf::error("cannot enable bus mastering {}", status);
    return zx::error(status);
  }

  if (zx_status_t status = e1000_pci_get_bti(pci, 0, adapter->bti.reset_and_get_address());
      status != ZX_OK) {
    fdf::error("failed to get BTI");
    return zx::error(status);
  }

  // Request 1 interrupt of any mode.
  pci_interrupt_mode_t irq_mode = PCI_INTERRUPT_MODE_DISABLED;
  if (zx_status_t status = e1000_pci_configure_interrupt_mode(pci, 1, &irq_mode); status != ZX_OK) {
    fdf::error("failed to configure irqs");
    return zx::error(status);
  }
  adapter->irq_mode = fuchsia_hardware_pci::InterruptMode(irq_mode);
  if (adapter->irq_mode.IsUnknown()) {
    fdf::error("unknown interrupt mode: {}", irq_mode);
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (zx_status_t status = e1000_pci_map_interrupt(pci, 0, adapter->irq.reset_and_get_address());
      status != ZX_OK) {
    fdf::error("failed to map irq");
    return zx::error(status);
  }

  if (zx::result<> result = IdentifyHardware(pci, adapter->hw); result.is_error()) {
    fdf::error("Failed to identify hardware: {}", result);
    return result;
  }

  switch (adapter->hw.mac.type) {
    case e1000_82573:
    case e1000_82583:
    case e1000_ich8lan:
    case e1000_ich9lan:
    case e1000_ich10lan:
    case e1000_pchlan:
    case e1000_pch2lan:
    case e1000_pch_lpt:
    case e1000_pch_spt:
    case e1000_82575:
    case e1000_82576:
    case e1000_82580:
    case e1000_i350:
    case e1000_i354:
    case e1000_i210:
    case e1000_i211:
    case e1000_vfadapt:
    case e1000_vfadapt_i350:
      adapter->has_amt = true;
      break;
    default:
      break;
  }

  adapter->has_manage = e1000_enable_mng_pass_thru(&adapter->hw);

  if (adapter->hw.mac.type >= igb_mac_min) {
    device_ = std::make_unique<Device<e1000_adv_rx_desc>>(*this, std::move(adapter),
                                                          std::move(compat_server));
  } else if (adapter->hw.mac.type >= em_mac_min) {
    device_ = std::make_unique<Device<e1000_rx_desc_extended>>(*this, std::move(adapter),
                                                               std::move(compat_server));
  } else {
    device_ = std::make_unique<Device<e1000_rx_desc>>(*this, std::move(adapter),
                                                      std::move(compat_server));
  }
  return device_->Start();
}

void Driver::Stop(fdf::StopCompleter completer) { device_->Stop(std::move(completer)); }

template <typename RxDescriptor>
Device<RxDescriptor>::Device(Driver& driver, std::unique_ptr<adapter>&& adapter,
                             std::unique_ptr<compat::SyncInitializedDeviceServer>&& compat_server)
    : DeviceBase(driver), adapter_(std::move(adapter)), compat_server_(std::move(compat_server)) {
  for (auto& buffer : adapter_->rx_buffers) {
    buffer = {
        .meta = {.port = kPortId, .frame_type = netdev::wire::FrameType::kEthernet},
        .data = {adapter_->rx_buffer_arena, 1},
    };
  }
}

template <typename RxDescriptor>
void Device<RxDescriptor>::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq_base,
                                     zx_status_t status, const zx_packet_interrupt_t* interrupt) {
  struct e1000_hw* hw = &adapter_->hw;
  if (status != ZX_OK) [[unlikely]] {
    fdf::error("irq wait failed? {}", zx_status_get_string(status));
    return;
  }

  auto ack_interrupt = fit::defer([&] {
    adapter_->irq.ack();
    if (adapter_->irq_mode == fuchsia_hardware_pci::InterruptMode::kLegacy) {
      if (zx_status_t status = e1000_pci_ack_interrupt(adapter_->osdep.pci); status != ZX_OK) {
        fdf::error("Failed to ack interrupt: {}", zx_status_get_string(status));
      }
    }
  });

  fdf::Arena arena('E1KD');

  const unsigned irq = E1000_READ_REG(hw, E1000_ICR);

  if (irq & E1000_ICR_RXT0) {
    // Rx timer intr (ring 0)

    uint16_t length = 0;
    std::lock_guard lock(adapter_->rx_mutex);

    CompleteRxTransaction transaction(*adapter_, arena);
    for (; rx_ring_.Available(&length); rx_ring_.Pop()) {
      transaction.Push(rx_ring_.HeadId(), length);
    }
    transaction.Commit();
  }
  if (irq & E1000_ICR_TXDW) {
    // Transmit desc written back

    std::lock_guard lock(adapter_->tx_mutex);

    CompleteTxTransaction transaction(*adapter_, arena);
    for (; tx_ring_.Available(); tx_ring_.Pop()) {
      transaction.Push(tx_ring_.HeadId(), ZX_OK);
    }
    transaction.Commit();
  }
  if (irq & E1000_ICR_LSC) {
    // Link status change
    adapter_->hw.mac.get_link_status = true;
    // Check link status asynchronously, this process can include sleeping and other delays, up to
    // and including a full reset of the device. It will still happen in sequence with the interrupt
    // handler (because it's using the same dispatcher) but it won't delay the interrupt ack.
    on_link_state_change_task_.Post(dispatcher);
  }
}

template <typename RxDescriptor>
void Device<RxDescriptor>::FlushTxQueue() {
  fdf::Arena arena('E1KD');

  CompleteTxTransaction transaction(*adapter_, arena);
  for (; !tx_ring_.IsEmpty(); tx_ring_.Pop()) {
    transaction.Push(tx_ring_.HeadId(), ZX_ERR_UNAVAILABLE);
  }
  transaction.Commit();
  tx_ring_.Clear();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::FlushRxSpace() {
  fdf::Arena arena('E1KD');

  CompleteRxTransaction transaction(*adapter_, arena);
  for (; !rx_ring_.IsEmpty(); rx_ring_.Pop()) {
    transaction.Push(rx_ring_.HeadId(), 0);
  }
  transaction.Commit();
  rx_ring_.Clear();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::UpdateOnlineStatus(bool perform_reset) {
  struct e1000_hw* hw = &adapter_->hw;
  bool link_check = false;

  /* Get the cached link value or read phy for real */
  switch (hw->phy.media_type) {
    case e1000_media_type_copper:
      if (hw->mac.get_link_status) {
        if (hw->mac.type == e1000_pch_spt) {
          msec_delay(50);
        }
        /* Do the work to read phy */
        e1000_check_for_link(hw);
        link_check = !hw->mac.get_link_status;
        if (link_check) { /* ESB2 fix */
          e1000_cfg_on_link_up(hw);
        }
      } else {
        link_check = true;
      }
      break;
    case e1000_media_type_fiber:
      e1000_check_for_link(hw);
      link_check = (E1000_READ_REG(hw, E1000_STATUS) & E1000_STATUS_LU);
      break;
    case e1000_media_type_internal_serdes:
      e1000_check_for_link(hw);
      link_check = hw->mac.serdes_has_link;
      break;
    /* VF device is type_unknown */
    case e1000_media_type_unknown:
      e1000_check_for_link(hw);
      link_check = !hw->mac.get_link_status;
      __FALLTHROUGH;
    default:
      break;
  }

  /* Now check for a transition */
  if (link_check && !online_) {
    uint16_t link_speed = 0;
    uint16_t link_duplex = 0;
    e1000_get_speed_and_duplex(hw, &link_speed, &link_duplex);
    /* Check if we must disable SPEED_MODE bit on PCI-E */
    if ((link_speed != SPEED_1000) &&
        ((hw->mac.type == e1000_82571) || (hw->mac.type == e1000_82572))) {
      int tarc0;
      tarc0 = E1000_READ_REG(hw, E1000_TARC(0));
      tarc0 &= ~TARC_SPEED_MODE_BIT;
      E1000_WRITE_REG(hw, E1000_TARC(0), tarc0);
    }

    /* Delay Link Up for Phy update */
    if (((hw->mac.type == e1000_i210) || (hw->mac.type == e1000_i211)) &&
        (hw->phy.id == I210_I_PHY_ID)) {
      msec_delay(I210_LINK_DELAY);
    }
    /* Reset if the media type changed. */
    if (hw->mac.type >= igb_mac_min && hw->dev_spec._82575.media_changed) {
      hw->dev_spec._82575.media_changed = false;
      adapter_->flags |= IGB_MEDIA_RESET;
      std::lock_guard lock(adapter_->tx_mutex);
      em_reset(adapter_.get(), tx_ring_);
      FlushTxQueue();
    }

    if (perform_reset && adapter_->was_reset) {
      e1000_clear_hw_cntrs_base_generic(&adapter_->hw);
      e1000_disable_ulp_lpt_lp(&adapter_->hw, true);

      {
        std::lock_guard lock(mac_filter_mutex_);
        SetMacFilterMode();
      }

      {
        std::lock_guard lock(adapter_->tx_mutex);
        InitializeTransmitUnit();
      }

      {
        // Flush all queued RX space to get a clean start after re-initializing.
        std::lock_guard lock(adapter_->rx_mutex);
        FlushRxSpace();
        InitializeReceiveUnit();
      }
      adapter_->was_reset = false;
    }

    online_ = true;

    fdf::info("Link is up {} Mbps {}", link_speed,
              ((link_duplex == FULL_DUPLEX) ? "Full Duplex" : "Half Duplex"));

    SendOnlineStatus(true);
  } else if (!link_check && online_) {
    fdf::info("Link is down");
    online_ = false;
    SendOnlineStatus(false);
    if (perform_reset) {
      {
        std::lock_guard lock(adapter_->tx_mutex);
        em_reset(adapter_.get(), tx_ring_);

        // Flush TX queue here since all those buffers are not going to be transmitted at this
        // point. Do not flush the RX space however, doing so will just allow it to be queued up
        // again making the flush pointless.
        FlushTxQueue();
      }

      // Reset will disable interrupts, enable them again so we get link status change interrupts.
      EnableInterrupts();
      adapter_->was_reset = true;
    }
  }

  /* Reset LAA into RAR[0] on 82571 */
  if (hw->mac.type == e1000_82571 && e1000_get_laa_state_82571(hw)) {
    e1000_rar_set(hw, hw->mac.addr, 0);
  }
}

template <typename RxDescriptor>
void Device<RxDescriptor>::OnLinkStateChange(async_dispatcher_t*, async::TaskBase*, zx_status_t) {
  UpdateOnlineStatus(true);
}

template <typename RxDescriptor>
void Device<RxDescriptor>::SendOnlineStatus(bool online) {
  if (!adapter_->ifc.is_valid()) {
    // This isn't an error. Status updates could happen before netdevice has fully started.
    return;
  }

  fdf::Arena arena('E1KD');
  auto builder = netdev::wire::PortStatus::Builder(arena);
  builder.mtu(kEthMtu).flags(online_ ? netdev::wire::StatusFlags::kOnline
                                     : netdev::wire::StatusFlags{});
  if (fidl::OneWayStatus status =
          adapter_->ifc.buffer(arena)->PortStatusChanged(kPortId, builder.Build());
      !status.ok()) {
    fdf::error("Failed to update port status: {}", status.FormatDescription());
    return;
  }
}

template <typename RxDescriptor>
zx::result<> Device<RxDescriptor>::AllocatePciResources() {
  if (zx_status_t status = e1000_pci_map_bar_buffer(
          adapter_->osdep.pci, 0u, ZX_CACHE_POLICY_UNCACHED_DEVICE, &adapter_->bar0_mmio);
      status != ZX_OK) {
    fdf::error("cannot map io {}", zx_status_get_string(status));
    return zx::error(status);
  }

  adapter_->osdep.membase = static_cast<MMIO_PTR volatile uint8_t*>(adapter_->bar0_mmio->get());
  // hw.hw_addr is never used for anything meaningful but it has to have a non-null value. There is
  // one location where it's used to compute the flash address but that address is never used so
  // this shouldn't matter. It used to be a pointer to the membase data member itself but that
  // doesn't seem to make any sense if it was pointing to some mapped flash.
  adapter_->hw.hw_addr = (u8*)adapter_->bar0_mmio->get();

  /* Only older adapters use IO mapping */
  if (adapter_->hw.mac.type < em_mac_min && adapter_->hw.mac.type > e1000_82543) {
    /* Figure our where our IO BAR is. We've already mapped the first BAR as
     * MMIO, so it must be one of the remaining five. */
    bool found_io_bar = false;
    fidl::Arena arena;
    for (uint32_t i = 1; i < pci::kMaxBarCount; i++) {
      pci_bar_t bar;
      if (zx_status_t status = e1000_pci_get_bar(adapter_->osdep.pci, i, &bar);
          status == ZX_OK && bar.type == PCI_BAR_TYPE_IO) {
        adapter_->osdep.iobase = bar.result.io.address;
        adapter_->hw.io_base = 0;
        found_io_bar = true;
        break;
      }
    }

    if (!found_io_bar) {
      fdf::error("Unable to locate IO BAR");
      return zx::error(ZX_ERR_IO_NOT_PRESENT);
    }
  }

  return zx::ok();
}

template <typename RxDescriptor>
zx::result<> Device<RxDescriptor>::SetupDescriptors() {
  const size_t rxd_ring_size = kRxDepth * sizeof(e1000_rx_desc);
  const size_t txd_ring_size = kTxDepth * sizeof(e1000_tx_desc);
  const size_t descriptor_buffer_size = txd_ring_size + rxd_ring_size;

  if (zx_status_t status = io_buffer_init(&adapter_->buffer, adapter_->bti.get(),
                                          descriptor_buffer_size, IO_BUFFER_RW | IO_BUFFER_CONTIG);
      status != ZX_OK) {
    fdf::error("cannot alloc io-buffer {}", status);
    return zx::error(status);
  }

  void* iomem = io_buffer_virt(&adapter_->buffer);
  zx_paddr_t iophys = io_buffer_phys(&adapter_->buffer);

  {
    std::lock_guard lock(adapter_->rx_mutex);
    memset(iomem, 0, rxd_ring_size);
    rx_ring_.AssignDescriptorMmio(iomem);
    adapter_->rxd_phys = iophys;
    iomem = PtrAdd(iomem, rxd_ring_size);
    iophys += rxd_ring_size;
  }

  {
    std::lock_guard lock(adapter_->tx_mutex);
    memset(iomem, 0, txd_ring_size);
    tx_ring_.AssignDescriptorMmio(iomem);
    adapter_->txd_phys = iophys;
    iomem = PtrAdd(iomem, txd_ring_size);
    iophys += txd_ring_size;
  }

  return zx::ok();
}

template <typename RxDescriptor>
zx::result<> Device<RxDescriptor>::Start() {
  DEBUGOUT("Start entry");

  vmo_store::Options options = {
      .map =
          vmo_store::MapOptions{
              .vm_option = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
              .vmar = nullptr,
          },
      .pin =
          vmo_store::PinOptions{
              .bti = adapter_->bti.borrow(),
              .bti_pin_options = ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE,
              .index = true,
          },
  };

  {
    fbl::AutoLock vmo_lock(&vmo_lock_);
    // Initialize VmoStore.
    vmo_store_ = std::make_unique<VmoStore>(options);

    if (zx_status_t status = vmo_store_->Reserve(fuchsia_hardware_network::wire::kMaxDataVmos);
        status != ZX_OK) {
      fdf::error("failed to reserve the capacity of VmoStore to max VMOs ({}): {}",
                 fuchsia_hardware_network::wire::kMaxDataVmos, zx_status_get_string(status));
      return zx::error(status);
    }
  }

  if (zx::result<> result = AllocatePciResources(); result.is_error()) {
    fdf::error("Allocation of PCI resources failed: {}", result);
    return result;
  }

  struct e1000_hw* hw = &adapter_->hw;

  /*
  ** For ICH8 and family we need to
  ** map the flash memory, and this
  ** must happen after the MAC is
  ** identified
  */
  if ((hw->mac.type == e1000_ich8lan) || (hw->mac.type == e1000_ich9lan) ||
      (hw->mac.type == e1000_ich10lan) || (hw->mac.type == e1000_pchlan) ||
      (hw->mac.type == e1000_pch2lan) || (hw->mac.type == e1000_pch_lpt)) {
    if (zx_status_t status =
            e1000_pci_map_bar_buffer(adapter_->osdep.pci, EM_BAR_TYPE_FLASH / 4,
                                     ZX_CACHE_POLICY_UNCACHED_DEVICE, &adapter_->flash_mmio);
        status != ZX_OK) {
      fdf::error("Mapping of Flash failed");
      return zx::error(status);
    }
    /* This is used in the shared code */
    hw->flash_address = (u8*)adapter_->flash_mmio->get();
    adapter_->osdep.flashbase = static_cast<MMIO_PTR uint8_t*>(adapter_->flash_mmio->get());
  }
  /*
  ** In the new SPT device flash is not  a
  ** separate BAR, rather it is also in BAR0,
  ** so use the same tag and an offset handle for the
  ** FLASH read/write macros in the shared code.
  */
  else if (hw->mac.type >= e1000_pch_spt) {
    adapter_->osdep.flashbase = adapter_->osdep.membase + E1000_FLASH_BASE_ADDR;
  }

  s32 err = e1000_setup_init_funcs(hw, TRUE);
  if (err) {
    fdf::error("Setup of Shared code failed, error {}", err);
    return zx::error(ZX_ERR_INTERNAL);
  }

  e1000_get_bus_info(hw);

  hw->mac.autoneg = 1;
  hw->phy.autoneg_wait_to_complete = FALSE;
  hw->phy.autoneg_advertised = (ADVERTISE_10_HALF | ADVERTISE_10_FULL | ADVERTISE_100_HALF |
                                ADVERTISE_100_FULL | ADVERTISE_1000_FULL);

  /* Copper options */
  if (hw->phy.media_type == e1000_media_type_copper) {
    hw->phy.mdix = 0;
    hw->phy.disable_polarity_correction = FALSE;
    hw->phy.ms_type = e1000_ms_hw_default;
  }

  /*
   * This controls when hardware reports transmit completion
   * status.
   */
  hw->mac.report_tx_early = 1;

  /* Check SOL/IDER usage */
  if (e1000_check_reset_block(hw)) {
    DEBUGOUT("PHY reset is blocked due to SOL/IDER session.");
  }

  /*
  ** Start from a known state, this is
  ** important in reading the nvm and
  ** mac from that.eth_queue_tx
  */
  e1000_reset_hw(&adapter_->hw);

  /* Make sure we have a good EEPROM before we read from it */
  if (e1000_validate_nvm_checksum(hw) < 0) {
    /*
    ** Some PCI-E parts fail the first check due to
    ** the link being in sleep state, call it again,
    ** if it fails a second time its a real issue.
    */
    if (e1000_validate_nvm_checksum(hw) < 0) {
      fdf::error("The EEPROM Checksum Is Not Valid");
      return zx::error(ZX_ERR_BAD_STATE);
    }
  }

  /* Copy the permanent MAC address out of the EEPROM */
  if (e1000_read_mac_addr(hw) < 0) {
    fdf::error("EEPROM read error while reading MAC address");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  {
    std::lock_guard lock(adapter_->tx_mutex);
    em_reset(adapter_.get(), tx_ring_);
  }

  adapter_->hw.mac.get_link_status = true;
  UpdateOnlineStatus(false);

  /* Non-AMT based hardware can now take control from firmware */
  if (adapter_->has_manage && !adapter_->has_amt) {
    em_get_hw_control(adapter_.get());
  }

  auto release_hw_control_on_failure = fit::defer([this]() {
    if (adapter_->has_manage && !adapter_->has_amt) {
      em_release_hw_control(adapter_.get());
    }
  });

  if (zx::result result = SetupDescriptors(); result.is_error()) {
    fdf::error("Failed to set up descriptors: {}", result);
    return result.take_error();
  }

  if (zx::result<> result = AddNetworkDevice(); result.is_error()) {
    fdf::error("Failed to add network device: {}", result);
    return result.take_error();
  }

  irq_handler_.set_object(adapter_->irq.get());
  if (zx_status_t status = irq_handler_.Begin(dispatcher()); status != ZX_OK) {
    fdf::error("failed to begin IRQ handling: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  release_hw_control_on_failure.cancel();

  DEBUGOUT("online");
  return zx::ok();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::Stop(fdf::StopCompleter completer) {
  irq_handler_.Cancel();
  on_link_state_change_task_.Cancel();

  em_release_hw_control(adapter_.get());

  e1000_reset_hw(&adapter_->hw);

  e1000_pci_set_bus_mastering(adapter_->osdep.pci, false);

  io_buffer_release(&adapter_->buffer);

  e1000_pci_free(adapter_->osdep.pci);

  adapter_.reset();

  completer(zx::ok());
}

template <typename RxDescriptor>
void Device<RxDescriptor>::InitializeTransmitUnit() {
  struct e1000_hw* hw = &adapter_->hw;
  u32 tctl, txdctl = 0, tarc, tipg = 0;

  DEBUGOUT("em_initialize_transmit_unit: begin");

  ZX_ASSERT(tx_ring_.IsEmpty());
  tx_ring_.Clear();

  const u64 bus_addr = adapter_->txd_phys;

  /* Base and Len of TX Ring */
  E1000_WRITE_REG(hw, E1000_TDLEN(0), kTxDepth * sizeof(struct e1000_tx_desc));
  E1000_WRITE_REG(hw, E1000_TDBAH(0), (u32)(bus_addr >> 32));
  E1000_WRITE_REG(hw, E1000_TDBAL(0), (u32)bus_addr);
  /* Init the HEAD/TAIL indices */
  E1000_WRITE_REG(hw, E1000_TDT(0), 0);
  E1000_WRITE_REG(hw, E1000_TDH(0), 0);

  DEBUGOUT("Base = %x, Length = %x", E1000_READ_REG(hw, E1000_TDBAL(0)),
           E1000_READ_REG(hw, E1000_TDLEN(0)));
  txdctl = 0;        /* clear txdctl */
  txdctl |= 0x1f;    /* PTHRESH */
  txdctl |= 1 << 8;  /* HTHRESH */
  txdctl |= 1 << 16; /* WTHRESH */
  txdctl |= 1 << 22; /* Reserved bit 22 must always be 1 */
  txdctl |= E1000_TXDCTL_GRAN;
  txdctl |= 1 << 25; /* LWTHRESH */

  E1000_WRITE_REG(hw, E1000_TXDCTL(0), txdctl);

  /* Set the default values for the Tx Inter Packet Gap timer */
  switch (hw->mac.type) {
    case e1000_80003es2lan:
      tipg = DEFAULT_82543_TIPG_IPGR1;
      tipg |= DEFAULT_80003ES2LAN_TIPG_IPGR2 << E1000_TIPG_IPGR2_SHIFT;
      break;
    case e1000_82542:
      tipg = DEFAULT_82542_TIPG_IPGT;
      tipg |= DEFAULT_82542_TIPG_IPGR1 << E1000_TIPG_IPGR1_SHIFT;
      tipg |= DEFAULT_82542_TIPG_IPGR2 << E1000_TIPG_IPGR2_SHIFT;
      break;
    default:
      if ((hw->phy.media_type == e1000_media_type_fiber) ||
          (hw->phy.media_type == e1000_media_type_internal_serdes)) {
        tipg = DEFAULT_82543_TIPG_IPGT_FIBER;
      } else {
        tipg = DEFAULT_82543_TIPG_IPGT_COPPER;
      }
      tipg |= DEFAULT_82543_TIPG_IPGR1 << E1000_TIPG_IPGR1_SHIFT;
      tipg |= DEFAULT_82543_TIPG_IPGR2 << E1000_TIPG_IPGR2_SHIFT;
  }

  E1000_WRITE_REG(hw, E1000_TIPG, tipg);
  E1000_WRITE_REG(hw, E1000_TIDV, 0);

  if (hw->mac.type >= e1000_82540)
    E1000_WRITE_REG(hw, E1000_TADV, 0);

  if ((hw->mac.type == e1000_82571) || (hw->mac.type == e1000_82572)) {
    tarc = E1000_READ_REG(hw, E1000_TARC(0));
    tarc |= TARC_SPEED_MODE_BIT;
    E1000_WRITE_REG(hw, E1000_TARC(0), tarc);
  } else if (hw->mac.type == e1000_80003es2lan) {
    /* errata: program both queues to unweighted RR */
    tarc = E1000_READ_REG(hw, E1000_TARC(0));
    tarc |= 1;
    E1000_WRITE_REG(hw, E1000_TARC(0), tarc);
    tarc = E1000_READ_REG(hw, E1000_TARC(1));
    tarc |= 1;
    E1000_WRITE_REG(hw, E1000_TARC(1), tarc);
  } else if (hw->mac.type == e1000_82574) {
    tarc = E1000_READ_REG(hw, E1000_TARC(0));
    tarc |= TARC_ERRATA_BIT;
    E1000_WRITE_REG(hw, E1000_TARC(0), tarc);
  }

  /* Program the Transmit Control Register */
  tctl = E1000_READ_REG(hw, E1000_TCTL);
  tctl &= ~E1000_TCTL_CT;
  tctl |= (E1000_TCTL_PSP | E1000_TCTL_RTLC | E1000_TCTL_EN |
           (E1000_COLLISION_THRESHOLD << E1000_CT_SHIFT));

  if (hw->mac.type >= e1000_82571)
    tctl |= E1000_TCTL_MULR;

  /* This write will effectively turn on the transmit unit. */
  E1000_WRITE_REG(hw, E1000_TCTL, tctl);

  /* SPT and KBL errata workarounds */
  if (hw->mac.type == e1000_pch_spt) {
    u32 reg;
    reg = E1000_READ_REG(hw, E1000_IOSFPC);
    reg |= E1000_RCTL_RDMTS_HEX;
    E1000_WRITE_REG(hw, E1000_IOSFPC, reg);
    /* i218-i219 Specification Update 1.5.4.5 */
    reg = E1000_READ_REG(hw, E1000_TARC(0));
    reg &= ~E1000_TARC0_CB_MULTIQ_3_REQ;
    reg |= E1000_TARC0_CB_MULTIQ_2_REQ;
    E1000_WRITE_REG(hw, E1000_TARC(0), reg);
  }
}

template <typename RxDescriptor>
void Device<RxDescriptor>::InitializeReceiveUnit() {
  struct e1000_hw* hw = &adapter_->hw;
  u32 rctl, rxcsum, rfctl;

  ZX_ASSERT(rx_ring_.IsEmpty());
  rx_ring_.Clear();

  /*
   * Make sure receives are disabled while setting
   * up the descriptor ring
   */
  rctl = E1000_READ_REG(hw, E1000_RCTL);
  /* Do not disable if ever enabled on this hardware */
  if ((hw->mac.type != e1000_82574) && (hw->mac.type != e1000_82583)) {
    E1000_WRITE_REG(hw, E1000_RCTL, rctl & ~E1000_RCTL_EN);
  }

  /* Setup the Receive Control Register */
  rctl &= ~(3 << E1000_RCTL_MO_SHIFT);
  rctl |= E1000_RCTL_EN | E1000_RCTL_BAM | E1000_RCTL_LBM_NO | E1000_RCTL_RDMTS_HALF |
          (hw->mac.mc_filter_type << E1000_RCTL_MO_SHIFT);

  /* Do not store bad packets */
  rctl &= ~E1000_RCTL_SBP;

  /* Disable Long Packet receive */
  rctl &= ~E1000_RCTL_LPE;

  /* Strip the CRC */
  rctl |= E1000_RCTL_SECRC;

  if (hw->mac.type >= e1000_82540) {
    E1000_WRITE_REG(hw, E1000_RADV, EM_RADV);

    /*
     * Set the interrupt throttling rate. Value is calculated
     * as DEFAULT_ITR = 1/(MAX_INTS_PER_SEC * 256ns)
     */
    E1000_WRITE_REG(hw, E1000_ITR, DEFAULT_ITR);
  }
  E1000_WRITE_REG(hw, E1000_RDTR, EM_RDTR);

  /* Use extended rx descriptor formats */
  rfctl = E1000_READ_REG(hw, E1000_RFCTL);
  rfctl |= E1000_RFCTL_EXTEN;
  /*
   * When using MSIX interrupts we need to throttle
   * using the EITR register (82574 only)
   */
  if (hw->mac.type == e1000_82574) {
    for (int i = 0; i < 4; i++) {
      E1000_WRITE_REG(hw, E1000_EITR_82574(i), DEFAULT_ITR);
    }

    /* Disable accelerated acknowledge */
    rfctl |= E1000_RFCTL_ACK_DIS;
  }
  E1000_WRITE_REG(hw, E1000_RFCTL, rfctl);

  rxcsum = E1000_READ_REG(hw, E1000_RXCSUM);
  rxcsum &= ~E1000_RXCSUM_TUOFL;

  E1000_WRITE_REG(hw, E1000_RXCSUM, rxcsum);

  /*
   * XXX TEMPORARY WORKAROUND: on some systems with 82573
   * long latencies are observed, like Lenovo X60. This
   * change eliminates the problem, but since having positive
   * values in RDTR is a known source of problems on other
   * platforms another solution is being sought.
   */
  if (hw->mac.type == e1000_82573) {
    E1000_WRITE_REG(hw, E1000_RDTR, 0x20);
  }

  /* Setup the Base and Length of the Rx Descriptor Ring */
  const u64 bus_addr = adapter_->rxd_phys;
  E1000_WRITE_REG(hw, E1000_RDLEN(0), kRxDepth * sizeof(union e1000_rx_desc_extended));
  E1000_WRITE_REG(hw, E1000_RDBAH(0), (u32)(bus_addr >> 32));
  E1000_WRITE_REG(hw, E1000_RDBAL(0), (u32)bus_addr);

  /*
   * Set PTHRESH for improved jumbo performance
   * According to 10.2.5.11 of Intel 82574 Datasheet,
   * RXDCTL(1) is written whenever RXDCTL(0) is written.
   * Only write to RXDCTL(1) if there is a need for different
   * settings.
   */

  if (hw->mac.type == e1000_82574) {
    u32 rxdctl = E1000_READ_REG(hw, E1000_RXDCTL(0));
    rxdctl |= 0x20;    /* PTHRESH */
    rxdctl |= 4 << 8;  /* HTHRESH */
    rxdctl |= 4 << 16; /* WTHRESH */
    rxdctl |= 1 << 24; /* Switch to granularity */
    E1000_WRITE_REG(hw, E1000_RXDCTL(0), rxdctl);
  } else if (hw->mac.type >= igb_mac_min) {
    u32 srrctl = 2048 >> E1000_SRRCTL_BSIZEPKT_SHIFT;
    rctl |= E1000_RCTL_SZ_2048;

    /* Setup the Base and Length of the Rx Descriptor Rings */
    u32 rxdctl;

    srrctl |= E1000_SRRCTL_DESCTYPE_ADV_ONEBUF;

    E1000_WRITE_REG(hw, E1000_RDLEN(0), kRxDepth * sizeof(struct e1000_rx_desc));
    E1000_WRITE_REG(hw, E1000_RDBAH(0), (uint32_t)(bus_addr >> 32));
    E1000_WRITE_REG(hw, E1000_RDBAL(0), (uint32_t)bus_addr);
    E1000_WRITE_REG(hw, E1000_SRRCTL(0), srrctl);
    /* Enable this Queue */
    rxdctl = E1000_READ_REG(hw, E1000_RXDCTL(0));
    rxdctl |= E1000_RXDCTL_QUEUE_ENABLE;
    rxdctl &= 0xFFF00000;
    rxdctl |= IGB_RX_PTHRESH;
    rxdctl |= IGB_RX_HTHRESH << 8;
    rxdctl |= IGB_RX_WTHRESH << 16;
    E1000_WRITE_REG(hw, E1000_RXDCTL(0), rxdctl);

    /* poll for enable completion */
    do {
      rxdctl = E1000_READ_REG(hw, E1000_RXDCTL(0));
    } while (!(rxdctl & E1000_RXDCTL_QUEUE_ENABLE));

  } else if (hw->mac.type >= e1000_pch2lan) {
    e1000_lv_jumbo_workaround_ich8lan(hw, FALSE);
  }

  /* Make sure VLAN Filters are off */
  rctl &= ~E1000_RCTL_VFE;

  if (hw->mac.type < igb_mac_min) {
    rctl |= E1000_RCTL_SZ_2048;
    rctl &= ~E1000_RCTL_BSEX;
    /* ensure we clear use DTYPE of 00 here */
    rctl &= ~0x00000C00;
  }

  /* Setup the Head and Tail Descriptor Pointers */
  E1000_WRITE_REG(hw, E1000_RDH(0), 0);
  E1000_WRITE_REG(hw, E1000_RDT(0), 0);

  /* Write out the settings */
  E1000_WRITE_REG(hw, E1000_RCTL, rctl);
}

template <typename RxDescriptor>
void Device<RxDescriptor>::EnableInterrupts() {
  E1000_WRITE_REG(&adapter_->hw, E1000_IMS, IMS_ENABLE_MASK);
  E1000_WRITE_FLUSH(&adapter_->hw);
}

template <typename RxDescriptor>
void Device<RxDescriptor>::DisableInterrupts() {
  // Write all bits in the interrupt mask clear register to disable all interrupts.
  E1000_WRITE_REG(&adapter_->hw, E1000_IMC, ~0u);
  E1000_WRITE_FLUSH(&adapter_->hw);
}

template <typename RxDescriptor>
zx::result<> Device<RxDescriptor>::AddNetworkDevice() {
  auto netdev_dispatcher =
      fdf::UnsynchronizedDispatcher::Create({}, "e1000-netdev", [](fdf_dispatcher_t*) {});
  if (netdev_dispatcher.is_error()) {
    fdf::error("Failed to create netdev dispatcher: {}", netdev_dispatcher);
    return netdev_dispatcher.take_error();
  }
  netdev_dispatcher_ = std::move(netdev_dispatcher.value());

  netdriver::Service::InstanceHandler handler(
      {.network_device_impl = [this](fdf::ServerEnd<netdriver::NetworkDeviceImpl> server_end) {
        fdf::BindServer(netdev_dispatcher_.get(), std::move(server_end), this);
      }});

  if (zx::result result = outgoing()->template AddService<netdriver::Service>(std::move(handler));
      result.is_error()) {
    fdf::error("Failed to add network device service to outgoing directory: {}", result);
    return result.take_error();
  }

  fidl::Arena arena;
  std::vector<fuchsia_driver_framework::wire::Offer> offers = compat_server_->CreateOffers2(arena);
  offers.push_back(fdf::MakeOffer2<netdriver::Service>(arena, component::kDefaultInstance));

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name("e1000")
                  .offers2(arena, std::move(offers))
                  .Build();

  auto endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create node controller endpoints: {}", endpoints);
    return endpoints.take_error();
  }

  if (fidl::WireResult result =
          fidl::WireCall(node())->AddChild(args, std::move(endpoints->server), {});
      !result.ok()) {
    fdf::error("Failed to add network device child: {}", result.FormatDescription());
    return zx::error(result.status());
  }

  controller_.Bind(std::move(endpoints->client), dispatcher());

  return zx::ok();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::Init(netdriver::wire::NetworkDeviceImplInitRequest* request,
                                fdf::Arena& arena, InitCompleter::Sync& completer) {
  adapter_->ifc.Bind(std::move(request->iface), driver_dispatcher()->get());

  auto endpoints = fdf::CreateEndpoints<netdriver::NetworkPort>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create network port endpoints: {}", endpoints);
    completer.buffer(arena).Reply(endpoints.status_value());
    return;
  }
  fdf::BindServer(netdev_dispatcher_.get(), std::move(endpoints->server), this);

  adapter_->ifc.buffer(arena)
      ->AddPort(kPortId, std::move(endpoints->client))
      .Then([completer = completer.ToAsync()](
                fdf::WireUnownedResult<netdriver::NetworkDeviceIfc::AddPort>& result) mutable {
        fdf::Arena arena(0u);
        if (!result.ok()) {
          fdf::error("Failed to add port: {}", result.FormatDescription());
        }
        completer.buffer(arena).Reply(result.status());
      });
}

template <typename RxDescriptor>
void Device<RxDescriptor>::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  e1000_clear_hw_cntrs_base_generic(&adapter_->hw);
  {
    std::lock_guard lock(mac_filter_mutex_);
    SetMacFilterMode();
  }

  /* AMT based hardware can now take control from firmware. */
  if (adapter_->has_manage && adapter_->has_amt) {
    em_get_hw_control(adapter_.get());
    std::lock_guard lock(adapter_->tx_mutex);
    em_reset(adapter_.get(), tx_ring_);
    FlushTxQueue();
  }

  e1000_power_up_phy(&adapter_->hw);
  e1000_disable_ulp_lpt_lp(&adapter_->hw, true);

  {
    std::lock_guard lock(adapter_->tx_mutex);
    InitializeTransmitUnit();
  }

  {
    std::lock_guard lock(adapter_->rx_mutex);
    FlushRxSpace();
    InitializeReceiveUnit();
  }

  EnableInterrupts();

  adapter_->hw.mac.get_link_status = true;
  UpdateOnlineStatus(false);

  started_ = true;
  completer.buffer(arena).Reply(ZX_OK);
}

template <typename RxDescriptor>
void Device<RxDescriptor>::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  started_ = false;
  DisableInterrupts();

  {
    std::lock_guard lock(adapter_->rx_mutex);
    FlushRxSpace();
  }

  {
    std::lock_guard lock(adapter_->tx_mutex);
    FlushTxQueue();
  }

  if (adapter_->has_manage && adapter_->has_amt) {
    em_release_hw_control(adapter_.get());
  }

  e1000_reset_hw(&adapter_->hw);
  if (adapter_->hw.mac.type >= e1000_82544) {
    E1000_WRITE_REG(&adapter_->hw, E1000_WUFC, 0);
  }

  completer.buffer(arena).Reply();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<netdriver::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) {
  fidl::WireTableBuilder builder = netdriver::wire::DeviceImplInfo::Builder(arena);

  builder.tx_depth(kTxDepth)
      .rx_depth(kRxDepth)
      .rx_threshold(kRxDepth / 2)
      .max_buffer_parts(1)
      .max_buffer_length(kMaxBufferLength)
      .buffer_alignment(kBufferAlignment)
      .min_rx_buffer_length(kMinRxBufferLength)
      .min_tx_buffer_length(kMinTxBufferLength)
      .tx_head_length(0)
      .tx_tail_length(0);

  completer.buffer(arena).Reply(builder.Build());
}

template <typename RxDescriptor>
void Device<RxDescriptor>::QueueTx(netdriver::wire::NetworkDeviceImplQueueTxRequest* request,
                                   fdf::Arena& arena, QueueTxCompleter::Sync& completer) {
  std::lock_guard lock(adapter_->tx_mutex);

  if (!started_.load(std::memory_order_relaxed) || !online_.load(std::memory_order_relaxed)) {
    // Device is either not started or not online, cannot transmit data at this time.
    CompleteTxTransaction transaction(*adapter_, arena);

    for (const auto& buffer : request->buffers) {
      transaction.Push(buffer.id, ZX_ERR_UNAVAILABLE);
    }
    transaction.Commit();
    return;
  }

  network::SharedAutoLock vmo_lock(&vmo_lock_);
  for (const auto& buffer : request->buffers) {
    const netdriver::wire::BufferRegion& region = buffer.data[0];

    const zx_paddr_t physical_addr = GetPhysicalAddress(region);

    const uint32_t next_tail = tx_ring_.Push(buffer.id, physical_addr, region.length);

    // Write the next tail to TDT, it should point one step past the last valid TX descriptor. The
    // tail returned by tx_ring_.Push matches this behavior.
    E1000_WRITE_REG(&adapter_->hw, E1000_TDT(0), next_tail);
  }
}

template <typename RxDescriptor>
void Device<RxDescriptor>::QueueRxSpace(
    netdriver::wire::NetworkDeviceImplQueueRxSpaceRequest* request, fdf::Arena& arena,
    QueueRxSpaceCompleter::Sync& completer) {
  network::SharedAutoLock vmo_lock(&vmo_lock_);
  std::lock_guard lock(adapter_->rx_mutex);
  for (const auto& buffer : request->buffers) {
    const netdriver::wire::BufferRegion& region = buffer.region;

    const zx_paddr_t physical_addr = GetPhysicalAddress(region);

    const uint32_t previous_tail = rx_ring_.Push(buffer.id, physical_addr);

    // Write the previous tail to RDT, it should point to the last valid RX descriptor. The tail
    // returned by rx_ring_.Push matches this behavior.
    E1000_WRITE_REG(&adapter_->hw, E1000_RDT(0), previous_tail);
  }
}

template <typename RxDescriptor>
void Device<RxDescriptor>::PrepareVmo(netdriver::wire::NetworkDeviceImplPrepareVmoRequest* request,
                                      fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  fbl::AutoLock vmo_lock(&vmo_lock_);
  size_t size;
  request->vmo.get_size(&size);
  if (zx_status_t status = vmo_store_->RegisterWithKey(request->id, std::move(request->vmo));
      status != ZX_OK) {
    fdf::error("Failed to register VMO {}: {}", request->id, zx_status_get_string(status));
    completer.buffer(arena).Reply(status);
    return;
  }
  completer.buffer(arena).Reply(ZX_OK);
}

template <typename RxDescriptor>
void Device<RxDescriptor>::ReleaseVmo(netdriver::wire::NetworkDeviceImplReleaseVmoRequest* request,
                                      fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  fbl::AutoLock vmo_lock(&vmo_lock_);
  zx::result<zx::vmo> status = vmo_store_->Unregister(request->id);
  if (status.status_value() != ZX_OK) {
    fdf::error("Failed to unregister VMO {}: {}", request->id, status);
  }
  completer.buffer(arena).Reply();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::GetInfo(
    fdf::Arena& arena, fdf::WireServer<netdriver::NetworkPort>::GetInfoCompleter::Sync& completer) {
  constexpr netdev::wire::FrameType kRxTypes[]{
      netdev::wire::FrameType::kEthernet,
  };
  constexpr netdev::wire::FrameTypeSupport kTxTypes[]{{
      .type = netdev::wire::FrameType::kEthernet,
      .features = netdev::wire::kFrameFeaturesRaw,
  }};

  fidl::WireTableBuilder builder = netdev::wire::PortBaseInfo::Builder(arena);
  builder.port_class(netdev::wire::PortClass::kEthernet).tx_types(kTxTypes).rx_types(kRxTypes);

  completer.buffer(arena).Reply(builder.Build());
}

template <typename RxDescriptor>
void Device<RxDescriptor>::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  auto builder = netdev::wire::PortStatus::Builder(arena);
  builder.mtu(kEthMtu).flags(online_ ? netdev::wire::StatusFlags::kOnline
                                     : netdev::wire::StatusFlags{});

  completer.buffer(arena).Reply(builder.Build());
}

template <typename RxDescriptor>
void Device<RxDescriptor>::SetActive(netdriver::wire::NetworkPortSetActiveRequest* request,
                                     fdf::Arena& arena, SetActiveCompleter::Sync& completer) {
  // Nothing to do.
}

template <typename RxDescriptor>
void Device<RxDescriptor>::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  zx::result endpoints = fdf::CreateEndpoints<netdriver::MacAddr>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create MacAddr endpoints: {}", endpoints);
    completer.Close(endpoints.error_value());
    return;
  }
  fdf::BindServer(netdev_dispatcher_.get(), std::move(endpoints->server), this);
  completer.buffer(arena).Reply(std::move(endpoints->client));
}

template <typename RxDescriptor>
void Device<RxDescriptor>::Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) {
  // The driver never explicitly removes the port, nothing to do here.
}

// MacAddr protocol:
template <typename RxDescriptor>
void Device<RxDescriptor>::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  fuchsia_net::wire::MacAddress mac;
  memcpy(mac.octets.data(), adapter_->hw.mac.addr, sizeof(adapter_->hw.mac.addr));
  completer.buffer(arena).Reply(mac);
}

template <typename RxDescriptor>
void Device<RxDescriptor>::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  fidl::WireTableBuilder builder = netdriver::wire::Features::Builder(arena);

  builder.multicast_filter_count(kMaxMulticastFilters)
      .supported_modes(netdriver::wire::SupportedMacFilterMode::kMulticastFilter |
                       netdriver::wire::SupportedMacFilterMode::kMulticastPromiscuous |
                       netdriver::wire::SupportedMacFilterMode::kPromiscuous);

  completer.buffer(arena).Reply(builder.Build());
}

template <typename RxDescriptor>
void Device<RxDescriptor>::SetMode(netdriver::wire::MacAddrSetModeRequest* request,
                                   fdf::Arena& arena, SetModeCompleter::Sync& completer) {
  {
    std::lock_guard lock(mac_filter_mutex_);
    for (size_t i = 0; i < request->multicast_macs.size(); ++i) {
      memcpy(&mac_filters_[i * ETH_ALEN], request->multicast_macs[i].octets.data(), ETH_ALEN);
    }
    mac_filter_mode_ = request->mode;
    mac_filters_.resize(request->multicast_macs.size() * ETH_ALEN);
    SetMacFilterMode();
  }
  completer.buffer(arena).Reply();
}

template <typename RxDescriptor>
void Device<RxDescriptor>::SetMacFilterMode() {
  if (adapter_->hw.mac.type == e1000_82542 && adapter_->hw.revision_id == E1000_REVISION_2) {
    uint32_t workaround_rctl = E1000_READ_REG(&adapter_->hw, E1000_RCTL);
    if (adapter_->hw.bus.pci_cmd_word & CMD_MEM_WRT_INVALIDATE) {
      e1000_pci_clear_mwi(&adapter_->hw);
    }
    workaround_rctl |= E1000_RCTL_RST;
    E1000_WRITE_REG(&adapter_->hw, E1000_RCTL, workaround_rctl);
    msec_delay(5);
    workaround_rctl = E1000_READ_REG(&adapter_->hw, E1000_RCTL);
    workaround_rctl &= ~(E1000_RCTL_MPE | E1000_RCTL_UPE | E1000_RCTL_SBP);
  }

  const uint32_t filter_count = static_cast<uint32_t>(mac_filters_.size() / ETH_ALEN);
  e1000_update_mc_addr_list(&adapter_->hw, mac_filters_.data(), filter_count);

  // Set all flags disabled, then enabled them as needed depending on the mode.
  uint32_t reg_rctl = E1000_READ_REG(&adapter_->hw, E1000_RCTL);
  reg_rctl &= ~(E1000_RCTL_MPE | E1000_RCTL_UPE | E1000_RCTL_SBP);

  switch (mac_filter_mode_) {
    case netdev::wire::MacFilterMode::kMulticastFilter:
      // Neither multicast nor unicast promiscuous should be enabled here
      break;
    case netdev::wire::MacFilterMode::kMulticastPromiscuous:
      reg_rctl |= E1000_RCTL_MPE;  // Multicast promiscuous
      break;
    case netdev::wire::MacFilterMode::kPromiscuous:
      reg_rctl |= (E1000_RCTL_MPE | E1000_RCTL_UPE);  // Multicast and unicast promiscuous
      break;
    default:
      // Nothing to do, we already disabled promiscuous mode above.
      break;
  }

  E1000_WRITE_REG(&adapter_->hw, E1000_RCTL, reg_rctl);

  if (adapter_->hw.mac.type == e1000_82542 && adapter_->hw.revision_id == E1000_REVISION_2) {
    uint32_t workaround_rctl = E1000_READ_REG(&adapter_->hw, E1000_RCTL);
    workaround_rctl &= ~E1000_RCTL_RST;
    E1000_WRITE_REG(&adapter_->hw, E1000_RCTL, workaround_rctl);
    msec_delay(5);
    if (adapter_->hw.bus.pci_cmd_word & CMD_MEM_WRT_INVALIDATE) {
      e1000_pci_set_mwi(&adapter_->hw);
    }
  }
}
template <typename RxDescriptor>
zx_paddr_t Device<RxDescriptor>::GetPhysicalAddress(
    const fuchsia_hardware_network_driver::wire::BufferRegion& region) {
  // Get physical address of buffers from region data.
  VmoStore::StoredVmo* stored_vmo = vmo_store_->GetVmo(region.vmo);
  ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %u", region.vmo);

  fzl::PinnedVmo::Region phy_region;
  size_t actual_regions = 0;
  zx_status_t status =
      stored_vmo->GetPinnedRegions(region.offset, region.length, &phy_region, 1, &actual_regions);
  ZX_ASSERT_MSG(status == ZX_OK, "failed to retrieve pinned region %s (actual=%zu)",
                zx_status_get_string(status), actual_regions);

  return phy_region.phys_addr;
}

}  // namespace e1000

// clang-format off
FUCHSIA_DRIVER_EXPORT2(::e1000::Driver);
