// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rtl8111.h"

#include <fuchsia/hardware/ethernet/cpp/banjo.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/io-buffer.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <zircon/types.h>

#include <ddktl/device.h>
#include <fbl/auto_lock.h>

// DISCLAIMER: This driver has not been tested since rewriting in C++. See fxr/727017 for details.

#define HI32(val) (((val) >> 32) & 0xffffffff)
#define LO32(val) ((val) & 0xffffffff)

typedef struct eth_desc {
  uint32_t status1;
  uint32_t status2;
  uint64_t data_addr;
} eth_desc_t;

class EthernetDevice;
using DeviceType = ddk::Device<EthernetDevice>;

class EthernetDevice : public DeviceType,
                       public ddk::EthernetImplProtocol<EthernetDevice, ddk::base_protocol> {
 public:
  explicit EthernetDevice(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Bind(void* ctx, zx_device_t* parent) {
    zxlogf(DEBUG, "rtl8111: binding device");

    zx_status_t status;
    auto edev = std::make_unique<EthernetDevice>(parent);
    if (!edev) {
      return ZX_ERR_NO_MEMORY;
    }
    mtx_init(&edev->lock_, mtx_plain);
    mtx_init(&edev->tx_lock_, mtx_plain);
    cnd_init(&edev->tx_cond_);

    edev->pci_ = ddk::Pci::FromFragment(parent);
    if (!edev->pci_.is_valid()) {
      return ZX_ERR_INTERNAL;
    }

    status = edev->pci_.ConfigureInterruptMode(1, &edev->irq_mode_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: failed to configure irqs");
      return status;
    }

    status = edev->pci_.MapInterrupt(0, &edev->irq_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: failed to map irq: %s", zx_status_get_string(status));
      return status;
    }

    status = edev->pci_.MapMmio(2u, ZX_CACHE_POLICY_UNCACHED_DEVICE, &edev->mmio_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: cannot map io: %s", zx_status_get_string(status));
      return status;
    }

    status = edev->pci_.SetBusMastering(true);
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: cannot enable bus master: %s", zx_status_get_string(status));
      return status;
    }

    status = edev->pci_.GetBti(0, &edev->bti_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: could not get bti: %s", zx_status_get_string(status));
      return status;
    }

    uint32_t mac_version = edev->mmio_->Read32(RTL_TCR) & 0x7cf00000;
    zxlogf(DEBUG, "rtl8111: version 0x%08x", mac_version);

    // TODO(stevensd): Don't require a contiguous buffer
    uint32_t alloc_size = ((ETH_BUF_SIZE + ETH_DESC_ELT_SIZE) * ETH_BUF_COUNT) * 2;
    status = edev->buffer_.Init(edev->bti_.get(), alloc_size, IO_BUFFER_RW | IO_BUFFER_CONTIG);
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: cannot alloc io-buffer: %s", zx_status_get_string(status));
      return status;
    }

    edev->InitBuffers();
    edev->InitRegs();

    status = edev->DdkAdd(ddk::DeviceAddArgs("rtl8111").set_proto_id(ZX_PROTOCOL_ETHERNET_IMPL));
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: failed to add device: %s", zx_status_get_string(status));
      return status;
    }

    status = thrd_create_with_name(
        &edev->irq_thread_,
        [](void* ctx) { return reinterpret_cast<EthernetDevice*>(ctx)->IrqThread(); }, &edev,
        "rtl-irq-thread");
    if (status != ZX_OK) {
      zxlogf(ERROR, "rtl8111: failed to create irq thread: %s", zx_status_get_string(status));
      return ZX_OK;  // The cleanup will be done in release
    }

    zxlogf(DEBUG, "rtl8111: bind successful");

    return ZX_OK;
  }

  void InitBuffers() {
    zxlogf(DEBUG, "rtl8111: Initializing buffers");
    txd_ring_ = reinterpret_cast<eth_desc_t*>(buffer_.virt());
    txd_phys_addr_ = buffer_.phys();
    txd_idx_ = 0;
    txb_ = static_cast<uint8_t*>(buffer_.virt()) + (2 * ETH_DESC_RING_SIZE);

    rxd_ring_ =
        reinterpret_cast<eth_desc_t*>(static_cast<uint8_t*>(buffer_.virt()) + ETH_DESC_RING_SIZE);
    rxd_phys_addr_ = buffer_.phys() + ETH_DESC_RING_SIZE;
    rxd_idx_ = 0;
    rxb_ = static_cast<uint8_t*>(txb_) + (ETH_BUF_SIZE * ETH_BUF_COUNT);

    uint64_t txb_phys = buffer_.phys() + (2 * ETH_DESC_RING_SIZE);
    uint64_t rxb_phys = txb_phys + (ETH_BUF_COUNT * ETH_BUF_SIZE);
    for (int i = 0; i < ETH_BUF_COUNT; i++) {
      bool is_end = i == (ETH_BUF_COUNT - 1);
      rxd_ring_[i].status1 = RX_DESC_OWN | (is_end ? RX_DESC_EOR : 0) | ETH_BUF_SIZE;
      rxd_ring_[i].status2 = 0;
      rxd_ring_[i].data_addr = rxb_phys;

      txd_ring_[i].status1 = 0;
      txd_ring_[i].status2 = 0;
      txd_ring_[i].data_addr = txb_phys;

      rxb_phys += ETH_BUF_SIZE;
      txb_phys += ETH_BUF_SIZE;
    }
  }

  void InitRegs() {
    zxlogf(DEBUG, "rtl8111: Initializing registers");

    // C+CR needs to be configured first - enable rx VLAN detagging and checksum offload
    mmio_->Write16(RTL_CPLUSCR,
                   mmio_->Read16(RTL_CPLUSCR) | RTL_CPLUSCR_RXVLAN | RTL_CPLUSCR_RXCHKSUM);

    // Reset the controller and wait for the operation to finish
    mmio_->Write8(RTL_CR, mmio_->Read8(RTL_CR) | RTL_CR_RST);
    while (mmio_->Read8(RTL_CR) & RTL_CR_RST) {
      zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));
    }

    // Unlock the configuration registers
    mmio_->Write8(RTL_9436CR,
                  (mmio_->Read8(RTL_9436CR) & RTL_9436CR_EEM_MASK) | RTL_9436CR_EEM_UNLOCK);

    // Set the tx and rx maximum packet size
    mmio_->Write8(RTL_MTPS, (mmio_->Read8(RTL_MTPS) & RTL_MTPS_MTPS_MASK) |
                                ZX_ROUNDUP(ETH_BUF_SIZE, 128) / 128);
    mmio_->Write16(RTL_RMS, (mmio_->Read16(RTL_RMS) & RTL_RMS_RMS_MASK) | ETH_BUF_SIZE);

    // Set the rx/tx descriptor ring addresses
    mmio_->Write32(RTL_RDSAR_LOW, LO32(rxd_phys_addr_));
    mmio_->Write32(RTL_RDSAR_HIGH, HI32(rxd_phys_addr_));
    mmio_->Write32(RTL_TNPDS_LOW, LO32(txd_phys_addr_));
    mmio_->Write32(RTL_TNPDS_HIGH, HI32(txd_phys_addr_));

    // Set the interframe gap and max DMA burst size in the tx config register
    uint32_t tcr = mmio_->Read32(RTL_TCR) & ~(RTL_TCR_IFG_MASK | RTL_TCR_MXDMA_MASK);
    mmio_->Write32(RTL_TCR, tcr | RTL_TCR_IFG96 | RTL_TCR_MXDMA_UNLIMITED);

    // Disable interrupts except link change and rx-ok and then clear all interrupts
    mmio_->Write16(RTL_IMR,
                   (mmio_->Read16(RTL_IMR) & ~RTL_INT_MASK) | RTL_INT_LINKCHG | RTL_INT_ROK);
    mmio_->Write16(RTL_ISR, 0xffff);

    // Lock the configuration registers and enable rx/tx
    mmio_->Write8(RTL_9436CR,
                  (mmio_->Read8(RTL_9436CR) & RTL_9436CR_EEM_MASK) | RTL_9436CR_EEM_LOCK);
    mmio_->Write8(RTL_CR, mmio_->Read8(RTL_CR) | RTL_CR_RE | RTL_CR_TE);

    // Configure the max dma burst, what types of packets we accept, and the multicast filter
    uint32_t rcr = mmio_->Read32(RTL_RCR) & ~(RTL_RCR_MXDMA_MASK | RTL_RCR_ACCEPT_MASK);
    mmio_->Write32(RTL_RCR, rcr | RTL_RCR_MXDMA_UNLIMITED | RTL_RCR_AB | RTL_RCR_AM | RTL_RCR_APM);
    mmio_->Write32(RTL_MAR7, 0xffffffff);  // Accept all multicasts
    mmio_->Write32(RTL_MAR3, 0xffffffff);

    // Read the MAC and link status
    uint32_t n = mmio_->Read32(RTL_MAC0);
    memcpy(mac_, &n, 4);
    n = mmio_->Read32(RTL_MAC1);
    memcpy(mac_ + 4, &n, 2);

    online_ = mmio_->Read8(RTL_PHYSTATUS) & RTL_PHYSTATUS_LINKSTS;

    zxlogf(INFO, "rtl111: mac address=%02x:%02x:%02x:%02x:%02x:%02x, link %s", mac_[0], mac_[1],
           mac_[2], mac_[3], mac_[4], mac_[5], online_ ? "online" : "offline");
  }

  int IrqThread() {
    while (1) {
      zx_status_t status = irq_.wait(nullptr);
      if (status != ZX_OK) {
        zxlogf(DEBUG, "rtl8111: irq wait failed: %s", zx_status_get_string(status));
        break;
      }

      fbl::AutoLock lock(&lock_);

      uint16_t isr = mmio_->Read16(RTL_ISR);
      if (isr & RTL_INT_LINKCHG) {
        bool was_online = online_;
        bool online = mmio_->Read8(RTL_PHYSTATUS) & RTL_PHYSTATUS_LINKSTS;
        if (online != was_online) {
          zxlogf(INFO, "rtl8111: link %s", online ? "online" : "offline");
          online_ = online;
          if (ifc_.ops) {
            ethernet_ifc_status(&ifc_, online ? ETHERNET_STATUS_ONLINE : 0);
          }
        }
      }
      if (isr & RTL_INT_TOK) {
        cnd_signal(&tx_cond_);
      }
      if (isr & RTL_INT_ROK) {
        eth_desc_t* rxd;
        while (!((rxd = rxd_ring_ + rxd_idx_)->status1 & RX_DESC_OWN)) {
          if (ifc_.ops) {
            size_t len = rxd->status1 & RX_DESC_LEN_MASK;
            ethernet_ifc_recv(&ifc_, static_cast<uint8_t*>(rxb_) + (rxd_idx_ * ETH_BUF_SIZE), len,
                              0);
          } else {
            zxlogf(ERROR, "rtl8111: No ethmac callback, dropping packet");
          }

          bool is_end = rxd_idx_ == (ETH_BUF_COUNT - 1);
          rxd->status1 = RX_DESC_OWN | (is_end ? RX_DESC_EOR : 0) | ETH_BUF_SIZE;

          rxd_idx_ = (rxd_idx_ + 1) % ETH_BUF_COUNT;
        }
      }

      mmio_->Write16(RTL_ISR, 0xffff);

      if (irq_mode_ == fuchsia_hardware_pci::InterruptMode::kLegacy) {
        pci_.AckInterrupt();
      }
    }
    return 0;
  }

  zx_status_t EthernetImplQuery(uint32_t options, ethernet_info_t* info) {
    if (options) {
      return ZX_ERR_INVALID_ARGS;
    }

    memset(info, 0, sizeof(*info));
    info->mtu = ETH_BUF_SIZE;
    memcpy(info->mac, mac_, sizeof(mac_));
    info->netbuf_size = sizeof(ethernet_netbuf_t);

    return ZX_OK;
  }

  void EthernetImplStop() {
    fbl::AutoLock lock(&lock_);
    ifc_.ops = NULL;
  }

  zx_status_t EthernetImplStart(const ethernet_ifc_protocol_t* ifc) {
    zx_status_t status = ZX_OK;

    fbl::AutoLock lock(&lock_);
    if (ifc_.ops) {
      status = ZX_ERR_BAD_STATE;
    } else {
      ifc_ = *ifc;
      ethernet_ifc_status(&ifc_, online_ ? ETHERNET_STATUS_ONLINE : 0);
    }

    return status;
  }

  void EthernetImplQueueTx(uint32_t options, ethernet_netbuf_t* netbuf,
                           ethernet_impl_queue_tx_callback completion_cb, void* cookie) {
    if (netbuf->data_size > (size_t)ETH_BUF_SIZE) {
      zxlogf(ERROR, "rtl8111: Unsupported packet length %zu", netbuf->data_size);
      completion_cb(cookie, ZX_ERR_INVALID_ARGS, netbuf);
      return;
    }
    uint32_t length = (uint32_t)netbuf->data_size;

    fbl::AutoLock tx_lock(&tx_lock_);

    if (txd_ring_[txd_idx_].status1 & TX_DESC_OWN) {
      fbl::AutoLock lock(&lock_);
      mmio_->Write16(RTL_IMR, mmio_->Read16(RTL_IMR) | RTL_INT_TOK);
      mmio_->Write16(RTL_ISR, RTL_INT_TOK);

      while (txd_ring_[txd_idx_].status1 & TX_DESC_OWN) {
        zxlogf(DEBUG, "rtl8111: Waiting for buffer");
        cnd_wait(&tx_cond_, &lock_);
      }

      mmio_->Write16(RTL_IMR, mmio_->Read16(RTL_IMR) & ~RTL_INT_TOK);
    }

    memcpy(static_cast<uint8_t*>(txb_) + (txd_idx_ * ETH_BUF_SIZE), netbuf->data_buffer, length);

    bool is_end = txd_idx_ == (ETH_BUF_COUNT - 1);
    txd_ring_[txd_idx_].status1 =
        (is_end ? TX_DESC_EOR : 0) | length | TX_DESC_OWN | TX_DESC_FS | TX_DESC_LS;

    mmio_->Write8(RTL_TPPOLL, mmio_->Read8(RTL_TPPOLL) | RTL_TPPOLL_NPQ);

    txd_idx_ = (txd_idx_ + 1) % ETH_BUF_COUNT;

    completion_cb(cookie, ZX_OK, netbuf);
  }

  zx_status_t SetPromisc(bool on) {
    if (on) {
      mmio_->Write16(RTL_RCR, mmio_->Read16(RTL_RCR | RTL_RCR_AAP));
    } else {
      mmio_->Write16(RTL_RCR, mmio_->Read16(RTL_RCR & ~RTL_RCR_AAP));
    }

    return ZX_OK;
  }

  zx_status_t EthernetImplSetParam(uint32_t param, int32_t value, const uint8_t* data,
                                   size_t data_size) {
    fbl::AutoLock lock(&lock_);
    switch (param) {
      case ETHERNET_SETPARAM_PROMISC:
        return SetPromisc((bool)value);
      default:
        return ZX_ERR_NOT_SUPPORTED;
    }
  }

  void EthernetImplGetBti(zx::bti* out_bti) {}

  void DdkRelease() {
    mmio_->Write8(RTL_CR, mmio_->Read8(RTL_CR) | RTL_CR_RST);
    pci_.SetBusMastering(false);
    delete this;
  }

 private:
  mtx_t lock_;
  mtx_t tx_lock_;
  cnd_t tx_cond_;
  ddk::Pci pci_;
  fuchsia_hardware_pci::InterruptMode irq_mode_;
  zx::interrupt irq_;
  std::optional<fdf::MmioBuffer> mmio_;
  thrd_t irq_thread_;
  zx::bti bti_;
  ddk::IoBuffer buffer_;

  eth_desc_t* txd_ring_;
  uint64_t txd_phys_addr_;
  int txd_idx_;
  void* txb_;

  eth_desc_t* rxd_ring_;
  uint64_t rxd_phys_addr_;
  int rxd_idx_;
  void* rxb_;

  uint8_t mac_[6];
  bool online_;

  ethernet_ifc_protocol_t ifc_;
};

static zx_driver_ops_t rtl8111_ethernet_driver_ops = {
    .version = DRIVER_OPS_VERSION,
    .bind = EthernetDevice::Bind,
};

ZIRCON_DRIVER(realtek_rtl8111, rtl8111_ethernet_driver_ops, "zircon", "0.1");
