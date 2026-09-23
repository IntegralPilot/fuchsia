// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/virtio-pci-device.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/bti.h>
#include <lib/zx/pmt.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <memory>
#include <span>

#include <fbl/alloc_checker.h>

#include "src/graphics/lib/virtio/virtio-abi.h"

namespace virtio_display {

// static
zx::result<std::unique_ptr<VirtioPciDevice>> VirtioPciDevice::Create(
    zx::bti bti, std::unique_ptr<virtio::Backend> backend) {
  // controlq setup.
  zx::vmo virtio_control_queue_buffer_pool_vmo;
  zx_status_t status = zx::vmo::create(zx_system_get_page_size(), /*options=*/0,
                                       &virtio_control_queue_buffer_pool_vmo);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate virtqueue controlq buffers VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  uint64_t virtio_control_queue_buffer_pool_size;
  status = virtio_control_queue_buffer_pool_vmo.get_size(&virtio_control_queue_buffer_pool_size);
  if (status != ZX_OK) {
    fdf::error("Failed to get virtqueue controlq buffers VMO size: {}", zx::make_result(status));
    return zx::error(status);
  }

  // Commit backing store and get the physical address.
  zx_paddr_t virtio_control_queue_buffer_pool_physical_address;
  zx::pmt virtio_control_queue_buffer_pool_pin;
  status = bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, virtio_control_queue_buffer_pool_vmo,
                   /*offset=*/0, virtio_control_queue_buffer_pool_size,
                   &virtio_control_queue_buffer_pool_physical_address, 1,
                   &virtio_control_queue_buffer_pool_pin);
  if (status != ZX_OK) {
    fdf::error("Failed to pin virtqueue controlq buffers VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  zx_vaddr_t virtio_control_queue_buffer_pool_begin;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, /*vmar_offset=*/0,
                                      virtio_control_queue_buffer_pool_vmo,
                                      /*vmo_offset=*/0, virtio_control_queue_buffer_pool_size,
                                      &virtio_control_queue_buffer_pool_begin);
  if (status != ZX_OK) {
    fdf::error("Failed to map virtqueue buffers VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  fdf::info(
      "Allocated virtqueue controlq buffers at virtual address 0x{:016x}, "
      "physical address 0x{:016x}",
      virtio_control_queue_buffer_pool_begin, virtio_control_queue_buffer_pool_physical_address);

  // NOLINTBEGIN(performance-no-int-to-ptr): Casting from zx_vaddr_t to a
  // pointer is unavoidable due to the zx::vmar::map() API.
  std::span<uint8_t> virtio_control_queue_buffer_pool(
      reinterpret_cast<uint8_t*>(virtio_control_queue_buffer_pool_begin),
      virtio_control_queue_buffer_pool_size);
  // NOLINTEND(performance-no-int-to-ptr)

  // cursorq setup.
  zx::vmo virtio_cursor_queue_buffer_pool_vmo;
  status = zx::vmo::create(zx_system_get_page_size(), /*options=*/0,
                           &virtio_cursor_queue_buffer_pool_vmo);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate virtqueue cursorq buffers VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  uint64_t virtio_cursor_queue_buffer_pool_size;
  status = virtio_cursor_queue_buffer_pool_vmo.get_size(&virtio_cursor_queue_buffer_pool_size);
  if (status != ZX_OK) {
    fdf::error("Failed to get virtqueue cursorq buffers VMO size: {}", zx::make_result(status));
    return zx::error(status);
  }

  // Commit backing store and get the physical address.
  zx_paddr_t virtio_cursor_queue_buffer_pool_physical_address;
  zx::pmt virtio_cursor_queue_buffer_pool_pin;
  status = bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, virtio_cursor_queue_buffer_pool_vmo,
                   /*offset=*/0, virtio_cursor_queue_buffer_pool_size,
                   &virtio_cursor_queue_buffer_pool_physical_address, 1,
                   &virtio_cursor_queue_buffer_pool_pin);
  if (status != ZX_OK) {
    fdf::error("Failed to pin virtqueue cursorq buffers VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  zx_vaddr_t virtio_cursor_queue_buffer_pool_begin;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, /*vmar_offset=*/0,
                                      virtio_cursor_queue_buffer_pool_vmo,
                                      /*vmo_offset=*/0, virtio_cursor_queue_buffer_pool_size,
                                      &virtio_cursor_queue_buffer_pool_begin);
  if (status != ZX_OK) {
    fdf::error("Failed to map virtqueue cursorq buffers VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  fdf::info(
      "Allocated virtqueue cursorq buffers at virtual address 0x{:016x}"
      ", physical address 0x{:016x}",
      virtio_cursor_queue_buffer_pool_begin, virtio_cursor_queue_buffer_pool_physical_address);

  // NOLINTBEGIN(performance-no-int-to-ptr): Casting from zx_vaddr_t to a
  // pointer is unavoidable due to the zx::vmar::map() API.
  std::span<uint8_t> virtio_cursor_queue_buffer_pool(
      reinterpret_cast<uint8_t*>(virtio_cursor_queue_buffer_pool_begin),
      virtio_cursor_queue_buffer_pool_size);
  // NOLINTEND(performance-no-int-to-ptr)

  fbl::AllocChecker alloc_checker;
  auto virtio_device = fbl::make_unique_checked<VirtioPciDevice>(
      &alloc_checker, std::move(bti), std::move(backend),
      std::move(virtio_control_queue_buffer_pool_vmo),
      std::move(virtio_control_queue_buffer_pool_pin),
      virtio_control_queue_buffer_pool_physical_address, virtio_control_queue_buffer_pool,
      std::move(virtio_cursor_queue_buffer_pool_vmo),
      std::move(virtio_cursor_queue_buffer_pool_pin),
      virtio_cursor_queue_buffer_pool_physical_address, virtio_cursor_queue_buffer_pool);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for VirtioPciDevice");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  status = virtio_device->Init();
  if (status != ZX_OK) {
    // VirtioPciDevice::Init() logs on error.
    return zx::error_result(status);
  }
  return zx::ok(std::move(virtio_device));
}

VirtioPciDevice::VirtioPciDevice(zx::bti bti, std::unique_ptr<virtio::Backend> backend,
                                 zx::vmo virtio_control_queue_buffer_pool_vmo,
                                 zx::pmt virtio_control_queue_buffer_pool_pin,
                                 zx_paddr_t virtio_control_queue_buffer_pool_physical_address,
                                 std::span<uint8_t> virtio_control_queue_buffer_pool,
                                 zx::vmo virtio_cursor_queue_buffer_pool_vmo,
                                 zx::pmt virtio_cursor_queue_buffer_pool_pin,
                                 zx_paddr_t virtio_cursor_queue_buffer_pool_physical_address,
                                 std::span<uint8_t> virtio_cursor_queue_buffer_pool)
    : virtio::Device(std::move(bti), std::move(backend)),
      virtio_control_queue_(this),
      virtio_cursor_queue_(this),
      virtio_control_queue_buffer_pool_vmo_(std::move(virtio_control_queue_buffer_pool_vmo)),
      virtio_cursor_queue_buffer_pool_vmo_(std::move(virtio_cursor_queue_buffer_pool_vmo)),
      virtio_control_queue_buffer_pool_pin_(std::move(virtio_control_queue_buffer_pool_pin)),
      virtio_cursor_queue_buffer_pool_pin_(std::move(virtio_cursor_queue_buffer_pool_pin)),
      virtio_control_queue_buffer_pool_physical_address_(
          virtio_control_queue_buffer_pool_physical_address),
      virtio_cursor_queue_buffer_pool_physical_address_(
          virtio_cursor_queue_buffer_pool_physical_address),
      virtio_control_queue_buffer_pool_(virtio_control_queue_buffer_pool),
      virtio_cursor_queue_buffer_pool_(virtio_cursor_queue_buffer_pool) {}

VirtioPciDevice::~VirtioPciDevice() {
  Release();

  if (!virtio_control_queue_buffer_pool_.empty()) {
    zx_vaddr_t virtio_control_queue_buffer_pool_begin =
        reinterpret_cast<zx_vaddr_t>(virtio_control_queue_buffer_pool_.data());
    zx::vmar::root_self()->unmap(virtio_control_queue_buffer_pool_begin,
                                 virtio_control_queue_buffer_pool_.size());
  }

  if (!virtio_cursor_queue_buffer_pool_.empty()) {
    zx_vaddr_t virtio_cursor_queue_buffer_pool_begin =
        reinterpret_cast<zx_vaddr_t>(virtio_cursor_queue_buffer_pool_.data());
    zx::vmar::root_self()->unmap(virtio_cursor_queue_buffer_pool_begin,
                                 virtio_cursor_queue_buffer_pool_.size());
  }
}

zx_status_t VirtioPciDevice::Init() {
  DeviceReset();

  virtio_abi::GpuDeviceConfig config;
  CopyDeviceConfig(&config, sizeof(config));
  fdf::trace("GpuDeviceConfig - pending events: 0x{:08x}", config.pending_events);
  fdf::trace("GpuDeviceConfig - scanout limit: {}", config.scanout_limit);
  fdf::trace("GpuDeviceConfig - capability set limit: {}", config.capability_set_limit);

  capability_set_limit_ = config.capability_set_limit;

  // Ack and set the driver status bit
  DriverStatusAck();

  const virtio_abi::GpuDeviceFeatures::Flags device_supported_features = DeviceFeaturesSupported();

  if ((device_supported_features & virtio_abi::GpuDeviceFeatures::kVirtioVersion1) == 0) {
    fdf::error("Legacy virtio (below 1.0) not supported by this driver");
    return ZX_ERR_NOT_SUPPORTED;
  }

  fdf::info("GPU device features: 0x{:016x}", device_supported_features);

  virtio_abi::GpuDeviceFeatures::Flags driver_supported_features =
      virtio_abi::GpuDeviceFeatures::kVirtioVersion1;
  if ((device_supported_features & virtio_abi::GpuDeviceFeatures::kGpuEdid) != 0) {
    driver_supported_features |= virtio_abi::GpuDeviceFeatures::kGpuEdid;
  }
  if ((device_supported_features & virtio_abi::GpuDeviceFeatures::kGpuResourceBlob) != 0) {
    driver_supported_features |= virtio_abi::GpuDeviceFeatures::kGpuResourceBlob;
  }

  DriverFeaturesAck(driver_supported_features);
  features_ = driver_supported_features;

  zx_status_t status = DeviceStatusFeaturesOk();
  if (status != ZX_OK) {
    fdf::error("Feature negotiation failed: {}", zx::make_result(status));
    return status;
  }

  // Allocate the control virtqueue.
  fbl::AutoLock virtio_control_queue_lock(&virtio_control_queue_mutex_);
  // virtio14 5.7.2 "Virtqueues": 0 controlq.
  status = virtio_control_queue_.Init(0, 16);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate controlq vring: {}", zx::make_result(status));
    return status;
  }

  fbl::AutoLock virtio_cursor_queue_lock(&virtio_cursor_queue_mutex_);
  // virtio14 5.7.2 "Virtqueues": 1 cursorq.
  status = virtio_cursor_queue_.Init(1, 16);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate cursor vring: {}", zx::make_result(status));
    return status;
  }

  StartIrqThread();
  DriverStatusOk();

  return ZX_OK;
}

void VirtioPciDevice::IrqRingUpdate() {
  fdf::trace("IrqRingUpdate()");

  // The lambda passed to virtio::Ring::IrqRingUpdate() should be annotated
  // __TA_REQUIRES(virtio_control_queue_mutex_). However, Clang's Thread Safety analysis
  // cannot prove the correctness of this annotation.
  //
  // Clang's static analyzer does not do inter-procedural analysis such as
  // inlining. Therefore, the analyzer does not know that IrqRingUpdate() only
  // calls the lambda argument before returning. So, it does not know that the
  // fbl::AutoLock is alive during the lambda calls, which would satisfy the
  // __TA_REQUIRES() annotation on the lambda.
  fbl::AutoLock virtio_control_queue_lock(&virtio_control_queue_mutex_);
  virtio_control_queue_.IrqRingUpdate(
      [&](vring_used_elem* used_buffer_info) __TA_NO_THREAD_SAFETY_ANALYSIS {
        const uint32_t used_descriptor_index = used_buffer_info->id;
        VirtioControlqBufferUsedByDevice(used_descriptor_index, used_buffer_info->len);
      });

  fbl::AutoLock virtio_cursor_queue_lock(&virtio_cursor_queue_mutex_);
  virtio_cursor_queue_.IrqRingUpdate(
      [&](vring_used_elem* used_buffer_info) __TA_NO_THREAD_SAFETY_ANALYSIS {
        const uint32_t used_descriptor_index = used_buffer_info->id;
        VirtioCursorqBufferUsedByDevice(used_descriptor_index);
      });
}

void VirtioPciDevice::IrqConfigChange() { fdf::trace("IrqConfigChange()"); }

const char* VirtioPciDevice::tag() const { return "virtio-gpu"; }

void VirtioPciDevice::VirtioControlqBufferUsedByDevice(uint32_t used_descriptor_index,
                                                       uint32_t chain_written_length) {
  if (unlikely(used_descriptor_index > std::numeric_limits<uint16_t>::max())) {
    fdf::warn("GPU device reported invalid used descriptor index: {}", used_descriptor_index);
    return;
  }
  // The check above ensures that the static_cast below does not lose any
  // information.
  uint16_t used_descriptor_index_u16 = static_cast<uint16_t>(used_descriptor_index);

  while (true) {
    struct vring_desc* buffer_descriptor =
        virtio_control_queue_.DescFromIndex(used_descriptor_index_u16);

    // Read information before the descriptor is freed.
    const bool next_descriptor_index_is_valid = (buffer_descriptor->flags & VRING_DESC_F_NEXT) != 0;
    const uint16_t next_descriptor_index = buffer_descriptor->next;
    virtio_control_queue_.FreeDesc(used_descriptor_index_u16);

    // virtio14 Section 2.7.7.1 "Driver Requirements: Used Buffer
    // Notification Suppression" requires that drivers handle spurious
    // notifications. So, we only notify the request thread when the device
    // reports having used the specific buffer that populated the request.
    // buffer has been used by the device.
    if (virtio_control_queue_request_response_.has_value() &&
        virtio_control_queue_request_response_->request_index == used_descriptor_index) {
      // TODO(costan): Ideally, the variable would be signaled when
      // `virtio_control_queue_mutex_` is not locked. This would avoid having the
      // ExchangeControlqRequestResponse() thread wake up, only to have to wait on the
      // mutex. Some options are:
      // * Plumb a (currently 1-element long) list of variables to be signaled
      //   from VirtioBufferUsedByDevice() to IrqRingUpdate().
      // * Replace virtio::Ring::IrqRingUpdate() with an iterator abstraction.
      //   The list of variables to be signaled would not need to be plumbed
      //   across methods.
      virtio_control_queue_request_response_->written_length = chain_written_length;
      virtio_control_queue_buffer_used_signal_.Signal();
    }

    if (!next_descriptor_index_is_valid) {
      break;
    }
    used_descriptor_index_u16 = next_descriptor_index;
  }
}

void VirtioPciDevice::VirtioCursorqBufferUsedByDevice(uint32_t used_descriptor_index) {
  if (unlikely(used_descriptor_index > std::numeric_limits<uint16_t>::max())) {
    fdf::warn("GPU device reported invalid used descriptor index: {}", used_descriptor_index);
    return;
  }
  // The check above ensures that the static_cast below does not lose any
  // information.
  uint16_t used_descriptor_index_u16 = static_cast<uint16_t>(used_descriptor_index);

  while (true) {
    struct vring_desc* buffer_descriptor =
        virtio_cursor_queue_.DescFromIndex(used_descriptor_index_u16);

    // Read information before the descriptor is freed.
    const bool next_descriptor_index_is_valid = (buffer_descriptor->flags & VRING_DESC_F_NEXT) != 0;
    const uint16_t next_descriptor_index = buffer_descriptor->next;
    virtio_cursor_queue_.FreeDesc(used_descriptor_index_u16);

    // virtio14 Section 2.7.7.1 "Driver Requirements: Used Buffer
    // Notification Suppression" requires that drivers handle spurious
    // notifications. So, we only notify the request thread when the device
    // reports having used the specific buffer that populated the request.
    // buffer has been used by the device.
    if (virtio_cursor_queue_request_index_.has_value() &&
        virtio_cursor_queue_request_index_.value() == used_descriptor_index) {
      virtio_cursor_queue_request_index_.reset();

      // TODO(costan): Ideally, the variable would be signaled when
      // `virtio_control_queue_mutex_` is not locked. This would avoid having the
      // ExchangeControlqRequestResponse() thread wake up, only to have to wait on the
      // mutex. Some options are:
      // * Plumb a (currently 1-element long) list of variables to be signaled
      //   from VirtioBufferUsedByDevice() to IrqRingUpdate().
      // * Replace virtio::Ring::IrqRingUpdate() with an iterator abstraction.
      //   The list of variables to be signaled would not need to be plumbed
      //   across methods.
      virtio_cursor_queue_buffer_used_signal_.Signal();
    }

    if (!next_descriptor_index_is_valid) {
      break;
    }
    used_descriptor_index_u16 = next_descriptor_index;
  }
}

void VirtioPciDevice::ExchangeControlqVariableLengthRequestResponse(
    std::span<const uint8_t> request, std::function<void(std::span<uint8_t>)> callback) {
  const size_t request_size = request.size();
  const size_t max_response_size = virtio_control_queue_buffer_pool_.size() - request_size;

  // Request/response exchanges are fully serialized.
  //
  // Relaxing this implementation constraint would require the following:
  // * An allocation scheme for `virtio_control_queue_buffer_pool`.
  // * A map of virtio queue descriptor index to condition variable, so multiple
  //   threads can be waiting on the device notification interrupt.
  // * Handling the case where `virtual_control_queue_buffer_pool` is full.
  //   Options are waiting on a new condition variable signaled whenever a
  //   buffer is released, and asserting that implies the buffer pool is
  //   statically sized to handle the maximum possible concurrency.
  //
  // Optionally, the locking around `virtio_control_queue_mutex_` could be more
  // fine-grained. Allocating the descriptors needs to be serialized, but
  // populating them can be done concurrently. Last, submitting the descriptors
  // to the virtio device must be serialized.
  fbl::AutoLock exhange_controlq_request_response_lock(&exchange_controlq_request_response_mutex_);

  fbl::AutoLock virtio_queue_lock(&virtio_control_queue_mutex_);

  // Allocate two virtqueue descriptors. This is the minimum number of
  // descriptors needed to represent a request / response exchange using the
  // split virtqueue format described in the virtio14 Section 2.7 "Split
  // Virtqueues". This is because each descriptor can point to a read-only or a
  // write-only memory buffer, and we need one of each.
  //
  // The first (returned) descriptor will point to the request buffer, and the
  // second (chained) descriptor will point to the response buffer.

  uint16_t request_descriptor_index;
  vring_desc* const request_descriptor =
      virtio_control_queue_.AllocDescChain(/*count=*/2, &request_descriptor_index);
  ZX_ASSERT(request_descriptor);
  virtio_control_queue_request_response_ = {request_descriptor_index, 0};

  std::span<uint8_t> request_span = virtio_control_queue_buffer_pool_.subspan(0, request_size);
  std::memcpy(request_span.data(), request.data(), request_size);

  const zx_paddr_t request_physical_address = virtio_control_queue_buffer_pool_physical_address_;
  request_descriptor->addr = request_physical_address;
  request_descriptor->len = static_cast<uint32_t>(request_size);
  ZX_ASSERT(request_descriptor->len == request_size);
  request_descriptor->flags = VRING_DESC_F_NEXT;

  vring_desc* const response_descriptor =
      virtio_control_queue_.DescFromIndex(request_descriptor->next);
  ZX_ASSERT(response_descriptor);

  std::span<uint8_t> response_span =
      virtio_control_queue_buffer_pool_.subspan(request_size, max_response_size);
  std::fill(response_span.begin(), response_span.end(), 0);

  const zx_paddr_t response_physical_address = request_physical_address + request_size;
  response_descriptor->addr = response_physical_address;
  response_descriptor->len = static_cast<uint32_t>(max_response_size);
  response_descriptor->flags = VRING_DESC_F_WRITE;

  fdf::trace("Sending {}-byte request descriptor {}", request_size, request_descriptor_index);

  // Submit the transfer & wait for the response
  virtio_control_queue_.SubmitChain(request_descriptor_index);
  virtio_control_queue_.Kick();

  virtio_control_queue_buffer_used_signal_.Wait(&virtio_control_queue_mutex_);

  ZX_ASSERT(virtio_control_queue_request_response_.has_value());
  auto written_length = virtio_control_queue_request_response_->written_length;

  virtio_control_queue_request_response_.reset();

  fdf::trace("Got written_length {}", written_length);

  callback(virtio_control_queue_buffer_pool_.subspan(request_size, written_length));
}

}  // namespace virtio_display
