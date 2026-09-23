// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/mmio-ptr/mmio-ptr.h>
#include <lib/virtio/backends/pci.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <mutex>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace {

// MMIO reads and writes are abstracted out into template methods that
// ensure fields are only accessed with the right size.
template <typename T>
void MmioWrite(MMIO_PTR volatile T* addr, T value) {
  T::bad_instantiation();
}

template <typename T>
void MmioRead(MMIO_PTR const volatile T* addr, T* value) {
  T::bad_instantiation();
}

template <>
void MmioWrite<uint32_t>(MMIO_PTR volatile uint32_t* addr, uint32_t value) {
  MmioWrite32(value, addr);
}

template <>
void MmioRead<uint32_t>(MMIO_PTR const volatile uint32_t* addr, uint32_t* value) {
  *value = MmioRead32(addr);
}

template <>
void MmioWrite<uint16_t>(MMIO_PTR volatile uint16_t* addr, uint16_t value) {
  MmioWrite16(value, addr);
}

template <>
void MmioRead<uint16_t>(MMIO_PTR const volatile uint16_t* addr, uint16_t* value) {
  *value = MmioRead16(addr);
}

template <>
void MmioWrite<uint8_t>(MMIO_PTR volatile uint8_t* addr, uint8_t value) {
  MmioWrite8(value, addr);
}

template <>
void MmioRead<uint8_t>(MMIO_PTR const volatile uint8_t* addr, uint8_t* value) {
  *value = MmioRead8(addr);
}

// Virtio 1.0 Section 4.1.3:
// 64-bit fields are to be treated as two 32-bit fields, with low 32 bit
// part followed by the high 32 bit part.
template <>
void MmioWrite<uint64_t>(MMIO_PTR volatile uint64_t* addr, uint64_t value) {
  auto words = reinterpret_cast<MMIO_PTR volatile uint32_t*>(addr);
  MmioWrite(&words[0], static_cast<uint32_t>(value));
  MmioWrite(&words[1], static_cast<uint32_t>(value >> 32));
}

template <>
void MmioRead<uint64_t>(MMIO_PTR const volatile uint64_t* addr, uint64_t* value) {
  auto words = reinterpret_cast<MMIO_PTR const volatile uint32_t*>(addr);
  uint32_t lo, hi;
  MmioRead(&words[0], &lo);
  MmioRead(&words[1], &hi);
  *value = static_cast<uint64_t>(lo) | (static_cast<uint64_t>(hi) << 32);
}

uint64_t GetOffset64(virtio_pci_cap64 cap64) {
  return (static_cast<uint64_t>(cap64.offset_hi) << 32) | cap64.cap.offset;
}

uint64_t GetLength64(virtio_pci_cap64 cap64) {
  return (static_cast<uint64_t>(cap64.length_hi) << 32) | cap64.cap.length;
}

}  // anonymous namespace

namespace virtio {

#define CHECK_RESULT(result)                                  \
  if ((result).is_error()) {                                  \
    if ((result).error_value().is_domain_error()) {           \
      return (result).error_value().domain_error();           \
    }                                                         \
    return (result).error_value().framework_error().status(); \
  }

// For reading the virtio specific vendor capabilities that can be PIO or MMIO space
#define cap_field(offset, field) static_cast<uint8_t>((offset) + offsetof(virtio_pci_cap_t, field))
zx_status_t PciModernBackend::ReadVirtioCap(uint8_t offset, virtio_pci_cap* cap) {
  fidl::Result result = fidl::Call(pci())->ReadConfig8(cap_field(offset, cap_vndr));
  CHECK_RESULT(result);
  cap->cap_vndr = result->value();
  result = fidl::Call(pci())->ReadConfig8(cap_field(offset, cap_next));
  CHECK_RESULT(result);
  cap->cap_next = result->value();
  result = fidl::Call(pci())->ReadConfig8(cap_field(offset, cap_len));
  CHECK_RESULT(result);
  cap->cap_len = result->value();
  result = fidl::Call(pci())->ReadConfig8(cap_field(offset, cfg_type));
  CHECK_RESULT(result);
  cap->cfg_type = result->value();
  result = fidl::Call(pci())->ReadConfig8(cap_field(offset, bar));
  CHECK_RESULT(result);
  cap->bar = result->value();
  result = fidl::Call(pci())->ReadConfig8(cap_field(offset, id));
  CHECK_RESULT(result);
  cap->id = result->value();

  fidl::Result result2 = fidl::Call(pci())->ReadConfig32(cap_field(offset, offset));
  CHECK_RESULT(result2);
  cap->offset = result2->value();
  result2 = fidl::Call(pci())->ReadConfig32(cap_field(offset, length));
  CHECK_RESULT(result)
  cap->length = result2->value();
  return ZX_OK;
}
#undef cap_field

zx_status_t PciModernBackend::ReadVirtioCap64(uint8_t cap_config_offset, virtio_pci_cap& cap,
                                              virtio_pci_cap64* cap64_out) {
  fidl::Result offset_hi =
      fidl::Call(pci())->ReadConfig32(cap_config_offset + sizeof(virtio_pci_cap_t));
  CHECK_RESULT(offset_hi);

  fidl::Result length_hi = fidl::Call(pci())->ReadConfig32(
      cap_config_offset + sizeof(virtio_pci_cap_t) + sizeof(offset_hi));
  CHECK_RESULT(length_hi);

  cap64_out->cap = cap;
  cap64_out->offset_hi = offset_hi->value();
  cap64_out->length_hi = length_hi->value();

  return ZX_OK;
}

zx_status_t PciModernBackend::Init() {
  std::lock_guard guard(lock());

  // try to parse capabilities
  fidl::Result capabilities =
      fidl::Call(pci())->GetCapabilities(fuchsia_hardware_pci::CapabilityId::kVendor);
  if (capabilities.is_error()) {
    return capabilities.error_value().status();
  }
  for (const auto& off : capabilities->offsets()) {
    virtio_pci_cap_t cap;

    zx_status_t st = ReadVirtioCap(off, &cap);
    if (st != ZX_OK) {
      zxlogf(ERROR, "Failed to read PCI capabilities");
      return st;
    }
    switch (cap.cfg_type) {
      case VIRTIO_PCI_CAP_COMMON_CFG:
        CommonCfgCallbackLocked(cap);
        break;
      case VIRTIO_PCI_CAP_NOTIFY_CFG: {
        // Virtio 1.0 section 4.1.4.4
        // notify_off_multiplier is a 32bit field following this capability
        fidl::Result result =
            fidl::Call(pci())->ReadConfig32(static_cast<uint8_t>(off + sizeof(virtio_pci_cap_t)));
        CHECK_RESULT(result);
        notify_off_mul_ = result->value();
        NotifyCfgCallbackLocked(cap);
        break;
      }
      case VIRTIO_PCI_CAP_ISR_CFG:
        IsrCfgCallbackLocked(cap);
        break;
      case VIRTIO_PCI_CAP_DEVICE_CFG:
        DeviceCfgCallbackLocked(cap);
        break;
      case VIRTIO_PCI_CAP_PCI_CFG:
        PciCfgCallbackLocked(cap);
        break;
      case VIRTIO_PCI_CAP_SHARED_MEMORY_CFG: {
        virtio_pci_cap64 cap64;
        if (zx_status_t st = ReadVirtioCap64(off, cap, &cap64); st != ZX_OK) {
          return st;
        }
        uint64_t offset = GetOffset64(cap64);
        uint64_t length = GetLength64(cap64);
        SharedMemoryCfgCallbackLocked(cap, offset, length);
        break;
      }
    }
  }

  // Ensure we found needed capabilities during parsing
  if (common_cfg_ == nullptr || isr_status_ == nullptr || device_cfg_ == 0 || notify_base_ == 0) {
    zxlogf(ERROR, "%s: failed to bind, missing capabilities", tag());
    return ZX_ERR_BAD_STATE;
  }

  zxlogf(TRACE, "virtio: modern pci backend successfully initialized");
  return ZX_OK;
}

// value pointers are used to maintain type safety with field width
void PciModernBackend::ReadDeviceConfig(uint16_t offset, uint8_t* value) {
  std::lock_guard guard(lock());
  MmioRead(reinterpret_cast<MMIO_PTR volatile uint8_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::ReadDeviceConfig(uint16_t offset, uint16_t* value) {
  std::lock_guard guard(lock());
  MmioRead(reinterpret_cast<MMIO_PTR volatile uint16_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::ReadDeviceConfig(uint16_t offset, uint32_t* value) {
  std::lock_guard guard(lock());
  MmioRead(reinterpret_cast<MMIO_PTR volatile uint32_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::ReadDeviceConfig(uint16_t offset, uint64_t* value) {
  std::lock_guard guard(lock());
  MmioRead(reinterpret_cast<MMIO_PTR volatile uint64_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::WriteDeviceConfig(uint16_t offset, uint8_t value) {
  std::lock_guard guard(lock());
  MmioWrite(reinterpret_cast<MMIO_PTR volatile uint8_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::WriteDeviceConfig(uint16_t offset, uint16_t value) {
  std::lock_guard guard(lock());
  MmioWrite(reinterpret_cast<MMIO_PTR volatile uint16_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::WriteDeviceConfig(uint16_t offset, uint32_t value) {
  std::lock_guard guard(lock());
  MmioWrite(reinterpret_cast<MMIO_PTR volatile uint32_t*>(device_cfg_ + offset), value);
}

void PciModernBackend::WriteDeviceConfig(uint16_t offset, uint64_t value) {
  std::lock_guard guard(lock());
  MmioWrite(reinterpret_cast<MMIO_PTR volatile uint64_t*>(device_cfg_ + offset), value);
}

// Attempt to map a bar found in a capability structure. If it has already been
// mapped and we have stored a valid handle in the structure then just return
// ZX_OK.
zx_status_t PciModernBackend::MapBar(uint8_t bar) {
  if (bar >= std::size(bar_)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (bar_[bar]) {
    return ZX_OK;
  }

  fidl::Result result = fidl::Call(pci())->GetBar(bar);
  CHECK_RESULT(result);

  if (result->result().result().Which() != fuchsia_hardware_pci::BarResult::Tag::kVmo) {
    return ZX_ERR_WRONG_TYPE;
  }

  auto& bar_result = result->result();

  zx::result mmio =
      fdf::MmioBuffer::Create(0, bar_result.size(), zx::vmo(bar_result.result().vmo()->release()),
                              ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (mmio.is_error()) {
    zxlogf(ERROR, "%s: Failed to map bar %u: %s", tag(), bar, mmio.status_string());
    return mmio.status_value();
  }

  bar_[bar] = std::move(*mmio);
  zxlogf(DEBUG, "%s: bar %u mapped to %p", tag(), bar, bar_[bar]->get());
  return ZX_OK;
}

void PciModernBackend::CommonCfgCallbackLocked(const virtio_pci_cap_t& cap) {
  zxlogf(DEBUG, "%s: common cfg found in bar %u offset %#x", tag(), cap.bar, cap.offset);
  if (MapBar(cap.bar) != ZX_OK) {
    return;
  }

  // Common config is a structure of type virtio_pci()common_cfg_t located at an
  // the bar and offset specified by the capability.
  auto addr = reinterpret_cast<uintptr_t>(bar_[cap.bar]->get()) + cap.offset;
  common_cfg_ = reinterpret_cast<MMIO_PTR volatile virtio_pci_common_cfg_t*>(addr);

  // Cache this when we find the config for kicking the queues later
}

void PciModernBackend::NotifyCfgCallbackLocked(const virtio_pci_cap_t& cap) {
  zxlogf(DEBUG, "%s: notify cfg found in bar %u offset %#x", tag(), cap.bar, cap.offset);
  if (MapBar(cap.bar) != ZX_OK) {
    return;
  }

  notify_base_ = reinterpret_cast<uintptr_t>(bar_[cap.bar]->get()) + cap.offset;
}

void PciModernBackend::IsrCfgCallbackLocked(const virtio_pci_cap_t& cap) {
  zxlogf(DEBUG, "%s: isr cfg found in bar %u offset %#x", tag(), cap.bar, cap.offset);
  if (MapBar(cap.bar) != ZX_OK) {
    return;
  }

  // interrupt status is directly read from the register at this address
  isr_status_ = reinterpret_cast<volatile uint32_t*>(
      reinterpret_cast<uintptr_t>(bar_[cap.bar]->get()) + cap.offset);
}

void PciModernBackend::DeviceCfgCallbackLocked(const virtio_pci_cap_t& cap) {
  zxlogf(DEBUG, "%s: device cfg found in bar %u offset %#x", tag(), cap.bar, cap.offset);
  if (MapBar(cap.bar) != ZX_OK) {
    return;
  }

  device_cfg_ = reinterpret_cast<uintptr_t>(bar_[cap.bar]->get()) + cap.offset;
}

void PciModernBackend::SharedMemoryCfgCallbackLocked(const virtio_pci_cap_t& cap, uint64_t offset,
                                                     uint64_t length) {
  if (MapBar(cap.bar) != ZX_OK) {
    return;
  }
  shared_memory_bar_ = cap.bar;
}

void PciModernBackend::PciCfgCallbackLocked(const virtio_pci_cap_t& cap) {
  // We are not using this capability presently since we can map the
  // bars for direct memory access.
}

// Get the ring size of a specific index
uint16_t PciModernBackend::GetRingSize(uint16_t index) {
  std::lock_guard guard(lock());

  uint16_t queue_size = 0;
  MmioWrite(&common_cfg_->queue_select, index);
  MmioRead(&common_cfg_->queue_size, &queue_size);
  zxlogf(TRACE, "QueueSize: %#x", queue_size);
  return queue_size;
}

// Set up ring descriptors with the backend.
zx_status_t PciModernBackend::SetRing(uint16_t index, uint16_t count, zx_paddr_t pa_desc,
                                      zx_paddr_t pa_avail, zx_paddr_t pa_used) {
  std::lock_guard guard(lock());

  // These offsets are wrong and this should be changed
  MmioWrite(&common_cfg_->queue_select, index);
  MmioWrite(&common_cfg_->queue_size, count);
  MmioWrite(&common_cfg_->queue_desc, pa_desc);
  MmioWrite(&common_cfg_->queue_avail, pa_avail);
  MmioWrite(&common_cfg_->queue_used, pa_used);

  if (irq_mode() == fuchsia_hardware_pci::InterruptMode::kMsiX) {
    uint16_t vector = 0;
    MmioWrite(&common_cfg_->config_msix_vector, PciBackend::kMsiConfigVector);
    MmioRead(&common_cfg_->config_msix_vector, &vector);
    if (vector != PciBackend::kMsiConfigVector) {
      zxlogf(ERROR, "MSI-X config vector in invalid state after write: %#x", vector);
      return ZX_ERR_BAD_STATE;
    }

    MmioWrite(&common_cfg_->queue_msix_vector, PciBackend::kMsiQueueVector);
    MmioRead(&common_cfg_->queue_msix_vector, &vector);
    if (vector != PciBackend::kMsiQueueVector) {
      zxlogf(ERROR, "MSI-X queue vector in invalid state after write: %#x", vector);
      return ZX_ERR_BAD_STATE;
    }
  }

  MmioWrite<uint16_t>(&common_cfg_->queue_enable, 1);
  // Assert that queue_notify_off is equal to the ring index.
  uint16_t queue_notify_off;
  MmioRead(&common_cfg_->queue_notify_off, &queue_notify_off);
  if (queue_notify_off != index) {
    zxlogf(ERROR, "Virtio queue notify setup failed");
    return ZX_ERR_BAD_STATE;
  }

  return ZX_OK;
}

void PciModernBackend::RingKick(uint16_t ring_index) {
  std::lock_guard guard(lock());

  // Virtio 1.0 Section 4.1.4.4
  // The address to notify for a queue is calculated using information from
  // the notify_off_multiplier, the capability's base + offset, and the
  // selected queue's offset.
  //
  // For performance reasons, we assume that the selected queue's offset is
  // equal to the ring index.
  auto addr = notify_base_ + ring_index * notify_off_mul_;
  auto ptr = reinterpret_cast<volatile uint16_t*>(addr);
  zxlogf(TRACE, "%s: kick %u addr %p", tag(), ring_index, ptr);
  *ptr = ring_index;
}

uint64_t PciModernBackend::ReadFeatures() {
  auto read_subset_features = [this](uint32_t select) {
    uint32_t val;
    {
      std::lock_guard guard(lock());
      MmioWrite(&common_cfg_->device_feature_select, select);
      MmioRead(&common_cfg_->device_feature, &val);
    }
    return val;
  };

  uint64_t bitmap = read_subset_features(1);
  bitmap = bitmap << 32 | read_subset_features(0);
  return bitmap;
}

void PciModernBackend::SetFeatures(uint64_t bitmap) {
  auto write_subset_features = [this](uint32_t select, uint32_t sub_bitmap) {
    std::lock_guard guard(lock());
    MmioWrite(&common_cfg_->driver_feature_select, select);
    uint32_t val;
    MmioRead(&common_cfg_->driver_feature, &val);
    MmioWrite(&common_cfg_->driver_feature, val | sub_bitmap);
    zxlogf(DEBUG, "%s: feature bits %08uh now set at offset %u", tag(), sub_bitmap, 32 * select);
  };

  uint32_t sub_bitmap = bitmap & UINT32_MAX;
  if (sub_bitmap) {
    write_subset_features(0, sub_bitmap);
  }
  sub_bitmap = bitmap >> 32;
  if (sub_bitmap) {
    write_subset_features(1, sub_bitmap);
  }
}

zx_status_t PciModernBackend::ConfirmFeatures() {
  std::lock_guard guard(lock());
  uint8_t val;

  MmioRead(&common_cfg_->device_status, &val);
  val |= VIRTIO_STATUS_FEATURES_OK;
  MmioWrite(&common_cfg_->device_status, val);

  // Check that the device confirmed our feature choices were valid
  MmioRead(&common_cfg_->device_status, &val);
  if ((val & VIRTIO_STATUS_FEATURES_OK) == 0) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  return ZX_OK;
}

void PciModernBackend::DeviceReset() {
  std::lock_guard guard(lock());

  MmioWrite<uint8_t>(&common_cfg_->device_status, 0u);
}

void PciModernBackend::WaitForDeviceReset() {
  std::lock_guard guard(lock());

  uint8_t device_status = 0xFF;
  while (device_status != 0) {
    MmioRead(&common_cfg_->device_status, &device_status);
  }
}

void PciModernBackend::DriverStatusOk() {
  std::lock_guard guard(lock());

  uint8_t device_status;
  MmioRead(&common_cfg_->device_status, &device_status);
  device_status |= VIRTIO_STATUS_DRIVER_OK;
  MmioWrite(&common_cfg_->device_status, device_status);
}

void PciModernBackend::DriverStatusAck() {
  std::lock_guard guard(lock());

  uint8_t device_status;
  MmioRead(&common_cfg_->device_status, &device_status);
  device_status |= VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER;
  MmioWrite(&common_cfg_->device_status, device_status);
}

uint32_t PciModernBackend::IsrStatus() {
  return (*isr_status_ & (VIRTIO_ISR_QUEUE_INT | VIRTIO_ISR_DEV_CFG_INT));
}

zx_status_t PciModernBackend::GetBarVmo(uint8_t bar_id, zx::vmo* vmo_out) {
  fidl::Result result = fidl::Call(pci())->GetBar(bar_id);
  CHECK_RESULT(result);

  CHECK_RESULT(result);

  if (result->result().result().Which() != fuchsia_hardware_pci::BarResult::Tag::kVmo) {
    return ZX_ERR_WRONG_TYPE;
  }

  *vmo_out = zx::vmo(result->result().result().vmo()->release());
  return ZX_OK;
}

zx_status_t PciModernBackend::GetSharedMemoryVmo(zx::vmo* vmo_out) {
  if (!shared_memory_bar_) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return GetBarVmo(*shared_memory_bar_, vmo_out);
}

}  // namespace virtio
