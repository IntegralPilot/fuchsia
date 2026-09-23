// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/virtio/backends/pci.h>
#include <zircon/syscalls/types.h>

#include <mutex>

#include <virtio/virtio.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

#if defined(__x86_64__) || defined(__i386__)
static inline uint8_t inp(uint16_t _port) {
  uint8_t rv;
  __asm__ __volatile__("inb %1, %0" : "=a"(rv) : "d"(_port));
  return (rv);
}

static inline uint16_t inpw(uint16_t _port) {
  uint16_t rv;
  __asm__ __volatile__("inw %1, %0" : "=a"(rv) : "d"(_port));
  return (rv);
}

static inline uint32_t inpd(uint16_t _port) {
  uint32_t rv;
  __asm__ __volatile__("inl %1, %0" : "=a"(rv) : "d"(_port));
  return (rv);
}

static inline void outp(uint16_t _port, uint8_t _data) {
  __asm__ __volatile__("outb %1, %0" : : "d"(_port), "a"(_data));
}

static inline void outpw(uint16_t _port, uint16_t _data) {
  __asm__ __volatile__("outw %1, %0" : : "d"(_port), "a"(_data));
}

static inline void outpd(uint16_t _port, uint32_t _data) {
  __asm__ __volatile__("outl %1, %0" : : "d"(_port), "a"(_data));
}
#else
static inline uint8_t inp(uint16_t _port) { return 0; }
static inline uint16_t inpw(uint16_t _port) { return 0; }
static inline uint32_t inpd(uint16_t _port) { return 0; }
static inline void outp(uint16_t _port, uint8_t _data) {}
static inline void outpw(uint16_t _port, uint16_t _data) {}
static inline void outpd(uint16_t _port, uint32_t _data) {}
#endif

namespace virtio {

void PciLegacyIoInterface::Read(uint16_t offset, uint8_t* val) const {
  *val = inp(static_cast<uint16_t>(offset));
}
void PciLegacyIoInterface::Read(uint16_t offset, uint16_t* val) const {
  *val = inpw(static_cast<uint16_t>(offset));
}
void PciLegacyIoInterface::Read(uint16_t offset, uint32_t* val) const {
  *val = inpd(static_cast<uint16_t>(offset));
}
void PciLegacyIoInterface::Write(uint16_t offset, uint8_t val) const {
  outp(static_cast<uint16_t>(offset), val);
}
void PciLegacyIoInterface::Write(uint16_t offset, uint16_t val) const {
  outpw(static_cast<uint16_t>(offset), val);
}
void PciLegacyIoInterface::Write(uint16_t offset, uint32_t val) const {
  outpd(static_cast<uint16_t>(offset), val);
}

zx_status_t PciLegacyBackend::Init() {
  std::lock_guard guard(lock());
  fidl::Result result = fidl::Call(pci())->GetBar(0u);
  if (result.is_error()) {
    zxlogf(ERROR, "%s: Couldn't get IO bar for device: %s", tag(),
           result.error_value().FormatDescription().c_str());
    if (result.error_value().is_domain_error()) {
      return result.error_value().domain_error();
    }
    return result.error_value().framework_error().status();
  }

  auto& bar0 = result->result();

  if (bar0.result().Which() != fuchsia_hardware_pci::BarResult::Tag::kIo) {
    return ZX_ERR_WRONG_TYPE;
  }

  bar0_base_ =
      static_cast<uint16_t>(bar0.result().io()->address() & std::numeric_limits<uint16_t>::max());

  zx_status_t status = zx_ioports_request(bar0.result().io()->resource().get(), bar0_base_,
                                          static_cast<uint32_t>(bar0.size()));
  if (status != ZX_OK) {
    return status;
  }

  device_cfg_offset_ = bar0_base_ + ((irq_mode() == fuchsia_hardware_pci::InterruptMode::kMsiX)
                                         ? VIRTIO_PCI_CONFIG_OFFSET_MSIX
                                         : VIRTIO_PCI_CONFIG_OFFSET_NOMSIX);
  zxlogf(DEBUG, "%s: using legacy backend (io base = %#04x, io size = %#04zx, device base = %#04x)",
         tag(), bar0_base_, bar0.size(), device_cfg_offset_);

  return ZX_OK;
}

// value pointers are used to maintain type safety with field width
void PciLegacyBackend::ReadDeviceConfig(uint16_t offset, uint8_t* value) {
  std::lock_guard guard(lock());
  legacy_io_->Read(static_cast<uint16_t>(device_cfg_offset_ + offset), value);
}

void PciLegacyBackend::ReadDeviceConfig(uint16_t offset, uint16_t* value) {
  std::lock_guard guard(lock());
  legacy_io_->Read(static_cast<uint16_t>(device_cfg_offset_ + offset), value);
}

void PciLegacyBackend::ReadDeviceConfig(uint16_t offset, uint32_t* value) {
  std::lock_guard guard(lock());
  legacy_io_->Read(static_cast<uint16_t>(device_cfg_offset_ + offset), value);
}

void PciLegacyBackend::ReadDeviceConfig(uint16_t offset, uint64_t* value) {
  std::lock_guard guard(lock());
  auto val = reinterpret_cast<uint32_t*>(value);

  legacy_io_->Read(static_cast<uint16_t>(device_cfg_offset_ + offset), &val[0]);
  legacy_io_->Read(static_cast<uint16_t>(device_cfg_offset_ + offset + sizeof(uint32_t)), &val[1]);
}

void PciLegacyBackend::WriteDeviceConfig(uint16_t offset, uint8_t value) {
  std::lock_guard guard(lock());
  legacy_io_->Write(static_cast<uint16_t>(device_cfg_offset_ + offset), value);
}

void PciLegacyBackend::WriteDeviceConfig(uint16_t offset, uint16_t value) {
  std::lock_guard guard(lock());
  legacy_io_->Write(static_cast<uint16_t>(device_cfg_offset_ + offset), value);
}

void PciLegacyBackend::WriteDeviceConfig(uint16_t offset, uint32_t value) {
  std::lock_guard guard(lock());
  legacy_io_->Write(static_cast<uint16_t>(device_cfg_offset_ + offset), value);
}
void PciLegacyBackend::WriteDeviceConfig(uint16_t offset, uint64_t value) {
  std::lock_guard guard(lock());
  auto words = reinterpret_cast<uint32_t*>(&value);
  legacy_io_->Write(static_cast<uint16_t>(device_cfg_offset_ + offset), words[0]);
  legacy_io_->Write(static_cast<uint16_t>(device_cfg_offset_ + offset + sizeof(uint32_t)),
                    words[1]);
}

// Get the ring size of a specific index
uint16_t PciLegacyBackend::GetRingSize(uint16_t index) {
  std::lock_guard guard(lock());
  uint16_t val;
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_QUEUE_SELECT, index);
  legacy_io_->Read(bar0_base_ + VIRTIO_PCI_QUEUE_SIZE, &val);
  zxlogf(TRACE, "%s: ring %u size = %u", tag(), index, val);
  return val;
}

// Set up ring descriptors with the backend.
zx_status_t PciLegacyBackend::SetRing(uint16_t index, uint16_t count, zx_paddr_t pa_desc,
                                      zx_paddr_t pa_avail, zx_paddr_t pa_used) {
  std::lock_guard guard(lock());
  // Virtio 1.0 section 2.4.2
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_QUEUE_SELECT, index);
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_QUEUE_SIZE, count);
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_QUEUE_PFN, static_cast<uint32_t>(pa_desc / 4096));

  // Virtio 1.0 section 4.1.4.8
  if (irq_mode() == fuchsia_hardware_pci::InterruptMode::kMsiX) {
    uint16_t vector = 0;
    legacy_io_->Write(bar0_base_ + VIRTIO_PCI_MSI_CONFIG_VECTOR, PciBackend::kMsiConfigVector);
    legacy_io_->Read(bar0_base_ + VIRTIO_PCI_MSI_CONFIG_VECTOR, &vector);
    if (vector != PciBackend::kMsiConfigVector) {
      zxlogf(ERROR, "MSI-X config vector in invalid state after write: %#x", vector);
      return ZX_ERR_BAD_STATE;
    }

    legacy_io_->Write(bar0_base_ + VIRTIO_PCI_MSI_QUEUE_VECTOR, PciBackend::kMsiQueueVector);
    legacy_io_->Read(bar0_base_ + VIRTIO_PCI_MSI_QUEUE_VECTOR, &vector);
    if (vector != PciBackend::kMsiQueueVector) {
      zxlogf(ERROR, "MSI-X queue vector in invalid state after write: %#x", vector);
      return ZX_ERR_BAD_STATE;
    }
  }

  zxlogf(TRACE, "%s: set ring %u (# = %u, addr = %#lx)", tag(), index, count, pa_desc);
  return ZX_OK;
}

void PciLegacyBackend::RingKick(uint16_t ring_index) {
  std::lock_guard guard(lock());
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_QUEUE_NOTIFY, ring_index);
  zxlogf(TRACE, "%s: kicked ring %u", tag(), ring_index);
}

uint64_t PciLegacyBackend::ReadFeatures() {
  std::lock_guard guard(lock());
  uint32_t val;
  legacy_io_->Read(bar0_base_ + VIRTIO_PCI_DEVICE_FEATURES, &val);
  // Legacy PCI back-end can only support one feature word.
  return uint64_t{val};
}

void PciLegacyBackend::SetFeatures(uint64_t bitmap) {
  // Legacy PCI back-end can only support one feature word.
  const uint32_t truncated_bitmap = bitmap & UINT32_MAX;

  std::lock_guard guard(lock());
  uint32_t val;
  legacy_io_->Read(bar0_base_ + VIRTIO_PCI_DRIVER_FEATURES, &val);
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_DRIVER_FEATURES, val | truncated_bitmap);
  zxlogf(DEBUG, "%s: feature bits %08uh now set", tag(), truncated_bitmap);
}

// Virtio v0.9.5 does not support the FEATURES_OK negotiation so this should
// always succeed.
zx_status_t PciLegacyBackend::ConfirmFeatures() { return ZX_OK; }

void PciLegacyBackend::DeviceReset() {
  std::lock_guard guard(lock());
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_DEVICE_STATUS, 0u);
  zxlogf(TRACE, "%s: device reset", tag());
}

void PciLegacyBackend::WaitForDeviceReset() {
  std::lock_guard guard(lock());
  uint8_t status = 0xFF;
  while (status != 0) {
    legacy_io_->Read(bar0_base_ + VIRTIO_PCI_DEVICE_STATUS, &status);
  }
  zxlogf(TRACE, "%s: device reset complete", tag());
}

void PciLegacyBackend::SetStatusBits(uint8_t bits) {
  std::lock_guard guard(lock());
  uint8_t status;
  legacy_io_->Read(bar0_base_ + VIRTIO_PCI_DEVICE_STATUS, &status);
  legacy_io_->Write(bar0_base_ + VIRTIO_PCI_DEVICE_STATUS, static_cast<uint8_t>(status | bits));
}

void PciLegacyBackend::DriverStatusAck() {
  SetStatusBits(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);
  zxlogf(TRACE, "%s: driver acknowledge", tag());
}

void PciLegacyBackend::DriverStatusOk() {
  SetStatusBits(VIRTIO_STATUS_DRIVER_OK);
  zxlogf(TRACE, "%s: driver ok", tag());
}

uint32_t PciLegacyBackend::IsrStatus() {
  std::lock_guard guard(lock());
  uint8_t isr_status;
  legacy_io_->Read(bar0_base_ + VIRTIO_PCI_ISR_STATUS, &isr_status);
  return isr_status & (VIRTIO_ISR_QUEUE_INT | VIRTIO_ISR_DEV_CFG_INT);
}

}  // namespace virtio
