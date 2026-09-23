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

#include "igc_driver.h"

#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <fidl/fuchsia.hardware.pci/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

#include <fbl/auto_lock.h>

// The io-buffer lib uses DFv1 logging which needs this symbol to link.
__EXPORT
__WEAK zx_driver_rec __zircon_driver_rec__ = {
    .ops = {},
    .driver = {},
};

namespace ethernet {
namespace igc {

namespace netdev = fuchsia_hardware_network;
namespace netdriver = fuchsia_hardware_network_driver;

constexpr char kNetDevDriverName[] = "igc-netdev";

constexpr static size_t kEthTxDescBufTotalSize = kEthTxDescRingCount * kEthTxDescSize;
constexpr static size_t kEthRxDescBufTotalSize = kEthRxBufCount * kEthRxDescSize;

constexpr static bool kDoAutoNeg = true;
constexpr static uint8_t kAutoAllMode = 0;

constexpr static uint16_t kIffPromisc = 0x100;
constexpr static uint16_t kIffAllmulti = 0x200;
constexpr static uint16_t kMaxNumMulticastAddresses = 128;

constexpr static uint64_t kMaxIntsPerSec = 8000;
constexpr static uint64_t kDefaultItr = (1000000000 / (kMaxIntsPerSec * 256));
constexpr static uint32_t kIgcRadvVal = 64;

constexpr static uint8_t kIgcRxPthresh = 8;
constexpr static uint8_t kIgcRxHthresh = 8;
constexpr static uint8_t kIgcRxWthresh = 4;

IgcDriver::IgcDriver() : fdf::DriverBase2("igc") {
  for (auto& buffer : adapter_->buffers) {
    buffer = {
        .meta{
            .port = kPortId,
            .frame_type = netdev::wire::FrameType::kEthernet,
        },
        .data{adapter_->rx_buffer_arena, 1u},
    };
  }
}

IgcDriver::~IgcDriver() { Shutdown(); }

void IgcDriver::Shutdown() {
  State expected = State::Running;
  if (!state_.compare_exchange_strong(expected, State::ShuttingDown)) {
    // Already shutting down or shut down.
    return;
  }
  // The driver release the control to firmware.
  uint32_t ctrl_ext = IGC_READ_REG(&adapter_->hw, IGC_CTRL_EXT);
  IGC_WRITE_REG(&adapter_->hw, IGC_CTRL_EXT, ctrl_ext & ~IGC_CTRL_EXT_DRV_LOAD);

  igc_reset_hw(&adapter_->hw);
  adapter_->osdep.pci.SetBusMastering(false);
  io_buffer_release(&adapter_->desc_buffer);

  irq_handler_.Cancel();

  zx_handle_close(adapter_->btih);
  zx_handle_close(adapter_->irqh);

  state_ = State::ShutDown;
}

zx_status_t IgcDriver::ConfigurePci(fdf::Namespace& incoming) {
  zx::result pci_client_end = incoming.Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_end.is_error()) {
    fdf::error("Failed to connect to PCI device: {}", pci_client_end);
    return pci_client_end.status_value();
  }

  adapter_->osdep.pci = ddk::Pci(std::move(pci_client_end.value()));
  auto& pci = adapter_->osdep.pci;

  if (!pci.is_valid()) {
    fdf::error("Failed to connect pci protocol.");
    return ZX_ERR_CONNECTION_REFUSED;
  }

  if (zx_status_t status = pci.SetBusMastering(true); status != ZX_OK) {
    fdf::error("cannot enable bus master {}", status);
    return status;
  }

  zx::bti bti;
  if (zx_status_t status = pci.GetBti(0, &bti); status != ZX_OK) {
    fdf::error("failed to get BTI");
    return status;
  }
  adapter_->btih = bti.release();

  // Request 1 interrupt of any mode.
  if (zx_status_t status = pci.ConfigureInterruptMode(1, &adapter_->irq_mode); status != ZX_OK) {
    fdf::error("failed to configure irqs");
    return status;
  }

  zx::interrupt interrupt;
  if (zx_status_t status = pci.MapInterrupt(0, &interrupt); status != ZX_OK) {
    fdf::error("failed to map irq");
    return status;
  }
  adapter_->irqh = interrupt.release();

  return ZX_OK;
}

zx::result<> IgcDriver::Start(fdf::DriverContext context) {
  dispatcher_ = driver_dispatcher();
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  auto netdev_dispatcher =
      fdf::UnsynchronizedDispatcher::Create({}, "igc-netdev", [](fdf_dispatcher_t*) {});
  if (netdev_dispatcher.is_error()) {
    fdf::error("Failed to create netdev dispatcher");
    return netdev_dispatcher.take_error();
  }
  netdev_dispatcher_ = std::move(netdev_dispatcher.value());

  return zx::make_result(Initialize(context, incoming));
}

void IgcDriver::Stop(fdf::StopCompleter completer) {
  Shutdown();
  completer(zx::ok());
}

zx_status_t IgcDriver::Initialize(fdf::DriverContext& context,
                                  std::shared_ptr<fdf::Namespace> incoming) {
  zx_status_t status = ZX_OK;

  // Set up PCI.
  if (status = ConfigurePci(*incoming); status != ZX_OK) {
    fdf::error("failed to configure PCI: {}", zx_status_get_string(status));
    return status;
  }

  vmo_store::Options options = {
      .map =
          vmo_store::MapOptions{
              .vm_option = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
              .vmar = nullptr,
          },
      .pin =
          vmo_store::PinOptions{
              .bti = zx::unowned_bti(adapter_->btih),
              .bti_pin_options = ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE,
              .index = true,
          },
  };

  {
    // Initialize VmoStore.
    fbl::AutoLock vmo_lock(&vmo_lock_);
    vmo_store_ = std::make_unique<VmoStore>(options);

    if (status = vmo_store_->Reserve(netdev::wire::kMaxDataVmos); status != ZX_OK) {
      fdf::error("failed to reserve the capacity of VmoStore to max VMOs ({}): {}",
                 netdev::wire::kMaxDataVmos, zx_status_get_string(status));
      return status;
    }
  }

  if (zx::result result =
          compat_server_.Initialize(incoming, outgoing(), context.node_name().value_or(""), name());
      result.is_error()) {
    fdf::error("Failed to initialize compatibility server: {}", result);
    return result.error_value();
  }

  // Get and store hardware infomation.
  IdentifyHardware();
  AllocatePCIResources();

  struct igc_hw* hw = &adapter_->hw;

  s32 error = igc_setup_init_funcs(hw, true);
  if (error) {
    // Keep this as debug print because this function returns error in tests, we don't want the
    // error log to fail tests. Change it to error log when there's a way to inject value to
    // simulated registers from FakePciProtocol.
    DEBUGOUT("Setup of Shared code failed, error %d\n", error);
  }

  igc_get_bus_info(hw);

  hw->mac.autoneg = kDoAutoNeg;
  hw->phy.autoneg_wait_to_complete = false;
  hw->phy.autoneg_advertised = ADVERTISE_10_HALF | ADVERTISE_10_FULL | ADVERTISE_100_HALF |
                               ADVERTISE_100_FULL | ADVERTISE_1000_FULL | ADVERTISE_2500_FULL;

  /* Copper options */
  if (hw->phy.media_type == igc_media_type_copper) {
    hw->phy.mdix = kAutoAllMode;
  }

  /* Check SOL/IDER usage */
  if (igc_check_reset_block(hw))
    fdf::error("PHY reset is blocked due to SOL/IDER session.");

  /*
  ** Start from a known state, this is
  ** important in reading the nvm and
  ** mac from that.
  */
  igc_reset_hw(hw);

  /* Make sure we have a good EEPROM before we read from it */
  if (igc_validate_nvm_checksum(hw) < 0) {
    /*
    ** Some PCI-E parts fail the first check due to
    ** the link being in sleep state, call it again,
    ** if it fails a second time its a real issue.
    */
    if (igc_validate_nvm_checksum(hw) < 0) {
      fdf::error("The EEPROM Checksum Is Not Valid");
      // TODO: Clean up states at these places.
      return ZX_ERR_INTERNAL;
    }
  }

  /* Copy the permanent MAC address out of the EEPROM */
  if (igc_read_mac_addr(hw) < 0) {
    fdf::error("EEPROM read error while reading MAC address");
    return ZX_ERR_INTERNAL;
  }

  if (!IsValidEthernetAddr(hw->mac.addr)) {
    fdf::error("Invalid MAC address");
    return ZX_ERR_INTERNAL;
  }

  // Initialize the descriptor buffer, map and pin to contigous phy addesses.
  if ((status = io_buffer_init(&adapter_->desc_buffer, adapter_->btih,
                               kEthTxDescBufTotalSize + kEthRxDescBufTotalSize,
                               IO_BUFFER_RW | IO_BUFFER_CONTIG)) != ZX_OK) {
    fdf::error("Failed initializing io_buffer: {}", zx_status_get_string(status));
    return status;
  }

  void* txrx_virt_addr = io_buffer_virt(&adapter_->desc_buffer);
  zx_paddr_t txrx_phy_addr = io_buffer_phys(&adapter_->desc_buffer);

  // Store the virtual and physical base addresses of Tx descriptor ring.
  adapter_->txdr = static_cast<struct igc_tx_desc*>(txrx_virt_addr);
  adapter_->txd_phys = txrx_phy_addr;

  // Store the virtual and physical base addresses of Rx descriptor ring. Following Tx addresses.
  adapter_->rxdr = reinterpret_cast<union igc_adv_rx_desc*>(adapter_->txdr + kEthTxDescRingCount);
  adapter_->rxd_phys = txrx_phy_addr + kEthTxDescBufTotalSize;

  u32 ctrl;
  ctrl = IGC_READ_REG(hw, IGC_CTRL);
  ctrl |= IGC_CTRL_VME;
  IGC_WRITE_REG(hw, IGC_CTRL, ctrl);

  // Clear all pending interrupts.
  IGC_READ_REG(hw, IGC_ICR);
  IGC_WRITE_REG(hw, IGC_ICS, IGC_ICS_LSC);

  // The driver can now take control from firmware.
  uint32_t ctrl_ext = IGC_READ_REG(hw, IGC_CTRL_EXT);
  IGC_WRITE_REG(hw, IGC_CTRL_EXT, ctrl_ext | IGC_CTRL_EXT_DRV_LOAD);

  irq_handler_.set_object(adapter_->irqh);
  if (status = irq_handler_.Begin(dispatcher()); status != ZX_OK) {
    fdf::error("Failed to begin IRQ handling: {}", zx_status_get_string(status));
    return status;
  }

  OnlineStatusUpdate();

  if (status = AddNetworkDevice(); status != ZX_OK) {
    fdf::error("Failed to add network device: {}", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

zx_status_t IgcDriver::AddNetworkDevice() {
  netdriver::Service::InstanceHandler handler(
      {.network_device_impl = [this](fdf::ServerEnd<netdriver::NetworkDeviceImpl> server_end) {
        netdev_binding_ = fdf::BindServer(netdev_dispatcher_.get(), std::move(server_end), this);
      }});

  if (zx::result result = outgoing()->AddService<netdriver::Service>(std::move(handler));
      result.is_error()) {
    fdf::error("Failed to add network device service to outgoing directory: {}", result);
    return result.error_value();
  }

  std::vector<fuchsia_driver_framework::Offer> offers = compat_server_.CreateOffers2();
  offers.push_back(fdf::MakeOffer2<netdriver::Service>(component::kDefaultInstance));

  auto child_result = AddChild(kNetDevDriverName,
                               std::span<const fuchsia_driver_framework::NodeProperty2>(), offers);
  if (child_result.is_error()) {
    fdf::error("Failed to add network device child: {}",
               zx_status_get_string(child_result.status_value()));
    return child_result.status_value();
  }

  controller_.Bind(std::move(child_result.value()), dispatcher());

  return ZX_OK;
}

void IgcDriver::IdentifyHardware() {
  ddk::Pci& pci = adapter_->osdep.pci;
  struct igc_hw* hw = &adapter_->hw;

  // Make sure our PCI configuration space has the necessary stuff set.
  pci.ReadConfig16(fuchsia_hardware_pci::Config::kCommand, &hw->bus.pci_cmd_word);

  fuchsia_hardware_pci::wire::DeviceInfo pci_info;
  zx_status_t status = pci.GetDeviceInfo(&pci_info);
  if (status != ZX_OK) {
    fdf::error("pci_get_device_info failure");
    return;
  }

  hw->vendor_id = pci_info.vendor_id;
  hw->device_id = pci_info.device_id;
  hw->revision_id = pci_info.revision_id;
  pci.ReadConfig16(fuchsia_hardware_pci::Config::kSubsystemVendorId, &hw->subsystem_vendor_id);
  pci.ReadConfig16(fuchsia_hardware_pci::Config::kSubsystemId, &hw->subsystem_device_id);

  // Do shared code init and setup.
  if (igc_set_mac_type(hw)) {
    fdf::error("igc_set_mac_type init failure");
    return;
  }
}

zx_status_t IgcDriver::AllocatePCIResources() {
  ddk::Pci& pci = adapter_->osdep.pci;

  // Map BAR0 memory
  zx_status_t status =
      pci.MapMmio(0u, ZX_CACHE_POLICY_UNCACHED_DEVICE, &adapter_->osdep.mmio_buffer);
  if (status != ZX_OK) {
    fdf::error("PCI cannot map io {}", status);
    return status;
  }

  adapter_->hw.hw_addr = (u8*)adapter_->osdep.mmio_buffer->get();
  adapter_->hw.back = &adapter_->osdep;
  return ZX_OK;
}

void IgcDriver::InitTransmitUnit() {
  struct igc_hw* hw = &adapter_->hw;

  u32 tctl, txdctl = 0;
  u64 bus_addr = adapter_->txd_phys;
  adapter_->txh_ind = 0;
  adapter_->txt_ind = 0;
  adapter_->txr_len = 0;

  // Base and length of TX Ring.
  IGC_WRITE_REG(hw, IGC_TDLEN(0), kEthTxDescRingCount * sizeof(struct igc_tx_desc));
  IGC_WRITE_REG(hw, IGC_TDBAH(0), (u32)(bus_addr >> 32));
  IGC_WRITE_REG(hw, IGC_TDBAL(0), (u32)bus_addr);
  // Init the HEAD/TAIL indices.
  IGC_WRITE_REG(hw, IGC_TDT(0), 0);
  IGC_WRITE_REG(hw, IGC_TDH(0), 0);

  DEBUGOUT("Base = %x, Length = %x\n", IGC_READ_REG(hw, IGC_TDBAL(0)),
           IGC_READ_REG(hw, IGC_TDLEN(0)));

  txdctl = 0;        /* clear txdctl */
  txdctl |= 0x1f;    /* PTHRESH */
  txdctl |= 1 << 8;  /* HTHRESH */
  txdctl |= 1 << 16; /* WTHRESH */
  txdctl |= 1 << 22; /* Reserved bit 22 must always be 1 */
  txdctl |= IGC_TXDCTL_GRAN;
  txdctl |= 1 << 25; /* LWTHRESH */

  IGC_WRITE_REG(hw, IGC_TXDCTL(0), txdctl);

  IGC_WRITE_REG(hw, IGC_TIDV, 1);
  IGC_WRITE_REG(hw, IGC_TADV, 64);

  // Program the Transmit Control Register.
  tctl = IGC_READ_REG(hw, IGC_TCTL);
  tctl &= ~IGC_TCTL_CT;
  tctl |= (IGC_TCTL_PSP | IGC_TCTL_RTLC | IGC_TCTL_EN | (IGC_COLLISION_THRESHOLD << IGC_CT_SHIFT));
  tctl |= IGC_TCTL_MULR;

  // This write will effectively turn on the transmit unit.
  IGC_WRITE_REG(hw, IGC_TCTL, tctl);
}

void IgcDriver::InitReceiveUnit() {
  struct igc_hw* hw = &adapter_->hw;
  u32 rctl = 0, rxcsum = 0, srrctl = 0, rxdctl = 0;

  // Make sure receives are disabled while setting up the descriptor ring.
  rctl = IGC_READ_REG(hw, IGC_RCTL);
  IGC_WRITE_REG(hw, IGC_RCTL, rctl & ~IGC_RCTL_EN);

  // Setup the Receive Control Register.
  rctl &= ~(3 << IGC_RCTL_MO_SHIFT);
  rctl |= IGC_RCTL_EN | IGC_RCTL_BAM | IGC_RCTL_LBM_NO | IGC_RCTL_RDMTS_HALF |
          (hw->mac.mc_filter_type << IGC_RCTL_MO_SHIFT);

  // Do not store bad packets.
  rctl &= ~IGC_RCTL_SBP;

  // Disable Long Packet receive.
  rctl &= ~IGC_RCTL_LPE;

  // Strip the CRC.
  rctl |= IGC_RCTL_SECRC;

  // Set the interrupt throttling rate. Value is calculated as DEFAULT_ITR = 1/(MAX_INTS_PER_SEC *
  // 256ns).

  IGC_WRITE_REG(hw, IGC_RADV, kIgcRadvVal);
  IGC_WRITE_REG(hw, IGC_ITR, kDefaultItr);
  IGC_WRITE_REG(hw, IGC_RDTR, 0);

  rxcsum = IGC_READ_REG(hw, IGC_RXCSUM);
  rxcsum &= ~IGC_RXCSUM_TUOFL;
  IGC_WRITE_REG(hw, IGC_RXCSUM, rxcsum);

  // Assume packet size won't be larger than MTU.
  srrctl |= 2048 >> IGC_SRRCTL_BSIZEPKT_SHIFT;
  rctl |= IGC_RCTL_SZ_2048;

  u64 bus_addr = adapter_->rxd_phys;
  adapter_->rxh_ind = 0;
  adapter_->rxt_ind = 0;
  adapter_->rxr_len = 0;

  srrctl |= IGC_SRRCTL_DESCTYPE_ADV_ONEBUF;

  IGC_WRITE_REG(hw, IGC_RDLEN(0), kEthRxBufCount * sizeof(union igc_adv_rx_desc));
  IGC_WRITE_REG(hw, IGC_RDBAH(0), (uint32_t)(bus_addr >> 32));
  IGC_WRITE_REG(hw, IGC_RDBAL(0), (uint32_t)bus_addr);
  IGC_WRITE_REG(hw, IGC_SRRCTL(0), srrctl);
  // Setup the HEAD/TAIL descriptor pointers.
  IGC_WRITE_REG(hw, IGC_RDH(0), 0);
  IGC_WRITE_REG(hw, IGC_RDT(0), 0);
  // Enable this Queue.
  rxdctl = IGC_READ_REG(hw, IGC_RXDCTL(0));
  rxdctl |= IGC_RXDCTL_QUEUE_ENABLE;
  rxdctl &= 0xFFF00000;
  rxdctl |= kIgcRxPthresh;
  rxdctl |= kIgcRxHthresh << 8;
  rxdctl |= kIgcRxWthresh << 16;
  IGC_WRITE_REG(hw, IGC_RXDCTL(0), rxdctl);

  do {
    rxdctl = IGC_READ_REG(hw, IGC_RXDCTL(0));
  } while (!(rxdctl & IGC_RXDCTL_QUEUE_ENABLE));

  // Make sure VLAN filters are off.
  rctl &= ~IGC_RCTL_VFE;

  // Write out the settings.
  IGC_WRITE_REG(hw, IGC_RCTL, rctl);
}

void IgcDriver::IfSetPromisc(uint32_t flags) {
  u32 reg_rctl;
  int mcnt = 0;
  struct igc_hw* hw = &adapter_->hw;

  reg_rctl = IGC_READ_REG(hw, IGC_RCTL);
  reg_rctl &= ~(IGC_RCTL_SBP | IGC_RCTL_UPE);
  if (flags & kIffAllmulti)
    mcnt = kMaxNumMulticastAddresses;

  // Don't disable if in MAX groups.
  if (mcnt < kMaxNumMulticastAddresses)
    reg_rctl &= (~IGC_RCTL_MPE);
  IGC_WRITE_REG(hw, IGC_RCTL, reg_rctl);

  if (flags & kIffPromisc) {
    reg_rctl |= (IGC_RCTL_UPE | IGC_RCTL_MPE);
    // Turn this on if you want to see bad packets.
    reg_rctl |= IGC_RCTL_SBP;
    IGC_WRITE_REG(hw, IGC_RCTL, reg_rctl);
  } else if (flags & kIffAllmulti) {
    reg_rctl |= IGC_RCTL_MPE;
    reg_rctl &= ~IGC_RCTL_UPE;
    IGC_WRITE_REG(hw, IGC_RCTL, reg_rctl);
  }
}

bool IgcDriver::IsValidEthernetAddr(uint8_t* addr) {
  const char zero_addr[6] = {0, 0, 0, 0, 0, 0};

  if ((addr[0] & 1) || (!memcmp(addr, zero_addr, kEtherAddrLen))) {
    return false;
  }

  return true;
}

void IgcDriver::ReapTxBuffers() {
  std::lock_guard lock(adapter_->tx_lock);
  // Assume TX depth here to simplify this and avoid having to deal with batching.
  static_assert(kEthTxBufCount < netdriver::wire::kMaxTxResults,
                "TX depth exceeds maximum TX results that can be completed");

  // Allocate the result array with max tx buffer pool size. Indicate a size for the arena so that
  // data is located on the stack whenever possible to avoid memory allocation.
  constexpr size_t kArenaSize =
      fidl::MaxSizeInChannel<netdriver::wire::NetworkDeviceIfcCompleteTxRequest,
                             fidl::MessageDirection::kSending>();
  fidl::Arena<kArenaSize> arena;

  fidl::VectorView<netdriver::wire::TxResult> results(arena, kEthTxBufCount);

  uint32_t& n = adapter_->txh_ind;

  size_t reap_count = 0;
  while (adapter_->txdr[n].upper.fields.status & IGC_TXD_STAT_DD) {
    adapter_->txdr[n].upper.fields.status = 0;
    results[reap_count].id = tx_buffers_[n].buffer_id;
    // We don't have a way to get the tx status of this packet from the tx descriptor since there
    // is no other STAT macros defined in FreeBSD driver,
    // so always return ZX_OK here.
    // TODO(https://fxbug.dev/42063016): Optimize it when we get the handbook.
    results[reap_count].status = ZX_OK;

    reap_count++;
    adapter_->txr_len--;
    n = (n + 1) & (kEthTxDescRingCount - 1);
  }
  if (reap_count > 0) {
    results.set_size(reap_count);
    fdf::Arena fdf_arena(0u);
    if (fidl::OneWayStatus status = adapter_->netdev_ifc.buffer(fdf_arena)->CompleteTx(results);
        !status.ok()) {
      fdf::error("Failed to complete TX: {}", status.FormatDescription());
      return;
    }
  }
}

bool IgcDriver::OnlineStatusUpdate() {
  bool new_status = IGC_READ_REG(&adapter_->hw, IGC_STATUS) & IGC_STATUS_LU;
  bool old_status = online_.exchange(new_status);
  return new_status != old_status;
}

// NetworkDevice::Callbacks implementations
void IgcDriver::Init(netdriver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
                     InitCompleter::Sync& completer) {
  adapter_->netdev_ifc.Bind(std::move(request->iface), driver_dispatcher()->get());

  auto endpoints = fdf::CreateEndpoints<netdriver::NetworkPort>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create network port endpoints: {}", endpoints);
    completer.buffer(arena).Reply(endpoints.status_value());
    return;
  }
  port_binding_ = fdf::BindServer(netdev_dispatcher_.get(), std::move(endpoints->server), this);
  adapter_->netdev_ifc.buffer(arena)
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

void IgcDriver::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  // Promiscuous settings.
  IfSetPromisc(kIffPromisc);
  igc_clear_hw_cntrs_base_generic(&adapter_->hw);

  igc_power_up_phy(&adapter_->hw);

  InitTransmitUnit();
  InitReceiveUnit();

  EnableInterrupt();

  OnlineStatusUpdate();

  started_ = true;
  completer.buffer(arena).Reply(ZX_OK);
}

void IgcDriver::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  started_ = false;
  DisableInterrupt();

  {
    // Reclaim all the buffers queued in rx descriptor ring.
    std::lock_guard lock(adapter_->rx_lock);
    // Assume RX depth here to simplify this and avoid having to deal with batching.
    static_assert(kEthRxBufCount < netdriver::wire::kMaxRxBuffers,
                  "RX depth exceeds maximum RX buffers that can be completed.");
    // Just use the provided arena here, no need to try to optimize this by trying to get everything
    // allocated on the stack. This is not part of the critical path.
    fidl::VectorView<netdriver::wire::RxBuffer> buffers(arena, kEthRxBufCount);

    size_t buf_idx = 0;
    uint32_t& rxh = adapter_->rxh_ind;

    while (adapter_->rxr_len > 0) {
      // Populate empty buffers.
      fidl::VectorView<netdriver::wire::RxBufferPart> parts(arena, 1);
      parts[0] = {.id = rx_buffers_[rxh].buffer_id, .offset = 0, .length = 0};

      buffers[buf_idx] = {
          .meta{
              .port = kPortId,
              .frame_type = netdev::wire::FrameType::kEthernet,
          },
          .data = parts,
      };

      rx_buffers_[rxh].available = false;
      // Clean up the status flag in rx descriptor.
      adapter_->rxdr[rxh].wb.upper.status_error = 0;
      rxh = (rxh + 1) & (kEthRxBufCount - 1);
      buf_idx++;
      adapter_->rxr_len--;
      ZX_ASSERT_MSG(buf_idx <= kEthRxBufCount, "buf_idx: %zu", buf_idx);
    }
    if (buf_idx > 0) {
      buffers.set_size(buf_idx);
      if (fidl::OneWayStatus status = adapter_->netdev_ifc.buffer(arena)->CompleteRx(buffers);
          !status.ok()) {
        fdf::error("Failed to complete RX buffers during stop: {}", status.FormatDescription());
        // Attempt to complete the rest of the stop operation. We can't indicate an error anyway.
      }
    }
  }

  {
    // Reclaim all tx buffers.
    std::lock_guard lock(adapter_->tx_lock);
    // Assume TX depth here to simplify this and avoid having to deal with batching.
    static_assert(kEthTxBufCount < netdriver::wire::kMaxTxResults,
                  "TX depth exceeds maximum TX results that can be completed");
    // Just use the provided arena here, no need to try to optimize this by trying to get everything
    // allocated on the stack. This is not part of the critical path.
    fidl::VectorView<netdriver::wire::TxResult> results(arena, kEthTxBufCount);

    size_t res_idx = 0;
    uint32_t& txh = adapter_->txh_ind;
    while (adapter_->txr_len > 0) {
      adapter_->txdr[txh].upper.fields.status = 0;
      results[res_idx].id = tx_buffers_[txh].buffer_id;
      results[res_idx].status = ZX_ERR_UNAVAILABLE;
      res_idx++;
      adapter_->txr_len--;
      txh = (txh + 1) & (kEthTxDescRingCount - 1);
    }
    if (res_idx > 0) {
      results.set_size(res_idx);
      if (fidl::OneWayStatus status = adapter_->netdev_ifc.buffer(arena)->CompleteTx(results);
          !status.ok()) {
        fdf::error("Failed to complete TX during stop: {}", status.FormatDescription());
      }
    }
  }

  igc_reset_hw(&adapter_->hw);
  IGC_WRITE_REG(&adapter_->hw, IGC_WUC, 0);

  completer.buffer(arena).Reply();
}

void IgcDriver::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<netdriver::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) {
  fidl::WireTableBuilder builder = netdriver::wire::DeviceImplInfo::Builder(arena);
  builder.tx_depth(kEthTxBufCount)
      .rx_depth(kEthRxBufCount)
      .rx_threshold(kEthRxBufCount / 2)
      .max_buffer_parts(1)
      .max_buffer_length(kEthRxTxMaxBufSize)
      .buffer_alignment(kEthRxTxBufAlignment)
      .min_rx_buffer_length(2048)
      .min_tx_buffer_length(60)
      .tx_head_length(0)
      .tx_tail_length(0);

  completer.buffer(arena).Reply(builder.Build());
}

void IgcDriver::QueueTx(netdriver::wire::NetworkDeviceImplQueueTxRequest* request,
                        fdf::Arena& arena, QueueTxCompleter::Sync& completer) {
  network::SharedAutoLock autp_lock(&vmo_lock_);
  std::lock_guard tx_lock(adapter_->tx_lock);
  if (!started_.load(std::memory_order_relaxed)) {
    if (request->buffers.empty()) {
      return;
    }

    // Assume TX depth here to simplify this and avoid having to deal with batching.
    static_assert(kEthTxBufCount < netdriver::wire::kMaxTxResults,
                  "TX depth exceeds maximum TX results that can be completed");
    fidl::VectorView<netdriver::wire::TxResult> results(arena, request->buffers.size());

    for (size_t i = 0; i < request->buffers.size(); ++i) {
      results[i].id = request->buffers[i].id;
      results[i].status = ZX_ERR_UNAVAILABLE;
    }
    if (fidl::OneWayStatus status = adapter_->netdev_ifc.buffer(arena)->CompleteTx(results);
        !status.ok()) {
      fdf::error("Failed to complete TX: {}", status.FormatDescription());
    }
    return;
  }

  uint32_t& n = adapter_->txt_ind;
  for (const auto& buffer : request->buffers) {
    ZX_ASSERT_MSG(buffer.data.size() == 1,
                  "Tx buffer contains multiple regions, which does not fit the configuration.");
    const netdriver::wire::BufferRegion& region = buffer.data[0];
    // Get physical address of buffers from region data.
    VmoStore::StoredVmo* stored_vmo = vmo_store_->GetVmo(region.vmo);
    ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %d", region.vmo);

    fzl::PinnedVmo::Region phy_region;
    size_t actual_regions = 0;
    zx_status_t status =
        stored_vmo->GetPinnedRegions(region.offset, region.length, &phy_region, 1, &actual_regions);
    ZX_ASSERT_MSG(status == ZX_OK, "failed to retrieve pinned region %s (actual=%zu)",
                  zx_status_get_string(status), actual_regions);

    // Modify tx descriptors and store buffer id.
    igc_tx_desc* cur_desc = &adapter_->txdr[n];
    tx_buffers_[n].buffer_id = buffer.id;
    cur_desc->buffer_addr = phy_region.phys_addr;
    cur_desc->lower.data =
        (uint32_t)(IGC_TXD_CMD_EOP | IGC_TXD_CMD_IFCS | IGC_TXD_CMD_RS | region.length);

    adapter_->txr_len++;
    // Update the tx descriptor ring tail index
    n = (n + 1) & (kEthTxDescRingCount - 1);
    // Update the last filled buffer to the adapter. This should point to one descriptor past the
    // last valid descriptor. Thus it's written after incrementing the tail.
    IGC_WRITE_REG(&adapter_->hw, IGC_TDT(0), n);
  }
}

void IgcDriver::QueueRxSpace(netdriver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
                             fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) {
  network::SharedAutoLock autp_lock(&vmo_lock_);
  std::lock_guard lock(adapter_->rx_lock);
  // Get current rx descriptor ring tail index.
  uint32_t& rxdr_tail = adapter_->rxt_ind;
  for (const auto& buffer : request->buffers) {
    const netdriver::wire::BufferRegion& vmo_region = buffer.region;

    // Store the buffer id info into internal buffer info array. It will be used by the interrupt
    // thread for reporting received buffers.
    rx_buffers_[rxdr_tail].buffer_id = buffer.id;
    // If this check fails, it means that the rx descriptor head/tail indexes are out of sync.
    ZX_ASSERT_MSG(rx_buffers_[rxdr_tail].available == false,
                  "All buffers are assigned as rx space.");
    rx_buffers_[rxdr_tail].available = true;

    VmoStore::StoredVmo* stored_vmo = vmo_store_->GetVmo(vmo_region.vmo);
    ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %d", vmo_region.vmo);

    fzl::PinnedVmo::Region phy_region;
    size_t actual_regions = 0;
    zx_status_t status = stored_vmo->GetPinnedRegions(vmo_region.offset, vmo_region.length,
                                                      &phy_region, 1, &actual_regions);
    ZX_ASSERT_MSG(status == ZX_OK, "failed to retrieve pinned region %s (actual=%zu)",
                  zx_status_get_string(status), actual_regions);

    // Make rx descriptor ready.
    union igc_adv_rx_desc* cur_desc = &adapter_->rxdr[rxdr_tail];
    cur_desc->read.pkt_addr = phy_region.phys_addr;
    ZX_ASSERT_MSG(phy_region.size >= kEthRxBufSize,
                  "Something went wrong physical buffer allocation.");
    cur_desc->wb.upper.status_error = 0;

    adapter_->rxr_len++;

    // Inform the hw the last ready-to-write buffer by setting the descriptor tail pointer. This
    // should point to the last valid descriptor. Thus it's written before incrementing the tail.
    IGC_WRITE_REG(&adapter_->hw, IGC_RDT(0), rxdr_tail);

    // Increase the tail index of rx descriptor ring, note that after this loop ends, rxdr_tail
    // should points to the descriptor right after the last ready-to-write buffer's descriptor.
    rxdr_tail = (rxdr_tail + 1) & (kEthRxBufCount - 1);
  }
}

void IgcDriver::PrepareVmo(netdriver::wire::NetworkDeviceImplPrepareVmoRequest* request,
                           fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  size_t size;
  request->vmo.get_size(&size);
  fbl::AutoLock vmo_lock(&vmo_lock_);
  zx_status_t status = vmo_store_->RegisterWithKey(request->id, std::move(request->vmo));
  if (status != ZX_OK) {
    fdf::error("Failed to store VMO(with mapping and pinning): {}", zx_status_get_string(status));
    completer.buffer(arena).Reply(status);
    return;
  }
  completer.buffer(arena).Reply(ZX_OK);
}

void IgcDriver::ReleaseVmo(netdriver::wire::NetworkDeviceImplReleaseVmoRequest* request,
                           fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  fbl::AutoLock vmo_lock(&vmo_lock_);
  zx::result<zx::vmo> status = vmo_store_->Unregister(request->id);
  if (status.status_value() != ZX_OK) {
    fdf::error("Failed to release VMO: {}", status);
  }
  completer.buffer(arena).Reply();
}

constexpr netdev::wire::FrameType kRxTypes[] = {netdev::wire::FrameType::kEthernet};
constexpr netdev::wire::FrameTypeSupport kTxTypes[] = {{
    .type = netdev::wire::FrameType::kEthernet,
    .features = netdev::wire::kFrameFeaturesRaw,
    .supported_flags = netdev::wire::TxFlags{},
}};

// NetworkPort protocol implementations.
void IgcDriver::GetInfo(
    fdf::Arena& arena, fdf::WireServer<netdriver::NetworkPort>::GetInfoCompleter::Sync& completer) {
  fidl::WireTableBuilder builder = netdev::wire::PortBaseInfo::Builder(arena);
  builder.port_class(netdev::wire::PortClass::kEthernet).tx_types(kTxTypes).rx_types(kRxTypes);

  completer.buffer(arena).Reply(builder.Build());
}

void IgcDriver::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  OnlineStatusUpdate();

  auto builder = netdev::wire::PortStatus::Builder(arena);
  builder.mtu(kEtherMtu).flags(online_ ? netdev::wire::StatusFlags::kOnline
                                       : netdev::wire::StatusFlags{});

  completer.buffer(arena).Reply(builder.Build());
}

void IgcDriver::SetActive(netdriver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
                          SetActiveCompleter::Sync& completer) { /* Do nothing here.*/ }

void IgcDriver::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  zx::result endpoints = fdf::CreateEndpoints<netdriver::MacAddr>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create MacAddr endpoints: {}", endpoints);
    completer.Close(endpoints.error_value());
    return;
  }
  mac_binding_ = fdf::BindServer(netdev_dispatcher_.get(), std::move(endpoints->server), this);
  completer.buffer(arena).Reply(std::move(endpoints->client));
}

void IgcDriver::Removed(
    fdf::Arena& arena,
    RemovedCompleter::Sync& completer) { /* Do nothing here, we don't remove port in this driver.*/
}

void IgcDriver::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  fuchsia_net::wire::MacAddress mac;
  memcpy(mac.octets.data(), adapter_->hw.mac.addr, kEtherAddrLen);
  completer.buffer(arena).Reply(mac);
}

void IgcDriver::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  fidl::WireTableBuilder builder = netdriver::wire::Features::Builder(arena);

  builder.multicast_filter_count(0).supported_modes(
      netdriver::wire::SupportedMacFilterMode::kPromiscuous);

  completer.buffer(arena).Reply(builder.Build());
}

void IgcDriver::SetMode(netdriver::wire::MacAddrSetModeRequest* request, fdf::Arena& arena,
                        SetModeCompleter::Sync& completer) {
  /* Do nothing here.*/
  completer.buffer(arena).Reply();
}

void IgcDriver::EnableInterrupt() {
  IGC_WRITE_REG(&adapter_->hw, IGC_IMS, IMS_ENABLE_MASK);
  IGC_WRITE_FLUSH(&adapter_->hw);
}

void IgcDriver::DisableInterrupt() {
  IGC_WRITE_REG(&adapter_->hw, IGC_IMC, 0xFFFFFFFFu);
  IGC_WRITE_FLUSH(&adapter_->hw);
}

void IgcDriver::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq_base,
                          zx_status_t status, const zx_packet_interrupt_t* interrupt) {
  if (status != ZX_OK) {
    if (status != ZX_ERR_CANCELED || state_ != State::ShuttingDown) {
      // Don't report ZX_ERR_CANCELED during shutdown, it's expected to happen.
      fdf::error("IRQ handler failed: {}", zx_status_get_string(status));
    }
    return;
  }

  auto ack_interrupt = fit::defer([&] {
    zx_interrupt_ack(adapter_->irqh);
    if (adapter_->irq_mode == fuchsia_hardware_pci::InterruptMode::kLegacy) {
      adapter_->osdep.pci.AckInterrupt();
    }
  });

  const unsigned irq = IGC_READ_REG(&adapter_->hw, IGC_ICR);
  if (irq & IGC_ICR_RXT0) {
    if (!started_.load(std::memory_order_relaxed)) {
      // skip the excessive rx interrupts if driver is stopped. The rx descriptor ring has been
      // cleaned up.
      return;
    }

    fdf::Arena arena('IGCD');

    std::lock_guard lock(adapter_->rx_lock);
    // The index for accessing "buffers" on a per interrupt basis.
    size_t buf_idx = 0;
    fidl::VectorView<netdriver::wire::RxBuffer>& buffers = adapter_->buffers;
    while ((adapter_->rxdr[adapter_->rxh_ind].wb.upper.status_error & IGC_RXD_STAT_DD)) {
      // Call complete_rx to pass the packet up to network device.
      buffers[buf_idx].data[0].id = rx_buffers_[adapter_->rxh_ind].buffer_id;
      buffers[buf_idx].data[0].length = adapter_->rxdr[adapter_->rxh_ind].wb.upper.length;
      buffers[buf_idx].data[0].offset = 0;

      // Release the buffer in internal buffer info array.
      rx_buffers_[adapter_->rxh_ind].available = false;

      // Update the head index of rx descriptor ring and the number of ring entries.
      adapter_->rxh_ind = (adapter_->rxh_ind + 1) & (kEthRxBufCount - 1);
      adapter_->rxr_len--;

      // Update buffer index and check whether the local buffer array is full.
      buf_idx++;
      if (buf_idx == adapter::kRxBuffersPerBatch) {  // Local buffer array is full.
        // Pass up a full batch of packets and reset buffer index.
        buffers.set_size(buf_idx);
        if (fidl::OneWayStatus status = adapter_->netdev_ifc.buffer(arena)->CompleteRx(buffers);
            !status.ok()) {
          fdf::error("Failed to complete RX: {}", status.FormatDescription());
          return;
        }
        buf_idx = 0;
      }
    }
    if (buf_idx != 0) {
      buffers.set_size(buf_idx);
      if (fidl::OneWayStatus status = adapter_->netdev_ifc.buffer(arena)->CompleteRx(buffers);
          !status.ok()) {
        fdf::error("Failed to complete RX: {}", status.FormatDescription());
        return;
      }
    }
  }
  if (irq & IGC_ICR_TXDW) {
    // Reap the tx buffers as much as possible when there is a tx write back interrupt.
    ReapTxBuffers();
  }
  if (irq & IGC_ICR_LSC) {
    if (OnlineStatusUpdate()) {
      fdf::Arena arena('IGCD');
      auto builder = netdev::wire::PortStatus::Builder(arena);
      builder.mtu(kEtherMtu).flags(online_ ? netdev::wire::StatusFlags::kOnline
                                           : netdev::wire::StatusFlags{});
      if (fidl::OneWayStatus status =
              adapter_->netdev_ifc.buffer(arena)->PortStatusChanged(kPortId, builder.Build());
          !status.ok()) {
        fdf::error("Failed to update port status: {}", status.FormatDescription());
        return;
      }
    }
  }
}

}  // namespace igc
}  // namespace ethernet

FUCHSIA_DRIVER_EXPORT2(::ethernet::igc::IgcDriver);
