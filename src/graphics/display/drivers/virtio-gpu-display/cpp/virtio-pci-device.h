// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_VIRTIO_PCI_DEVICE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_VIRTIO_PCI_DEVICE_H_

#include <lib/driver/logging/cpp/logger.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>
#include <lib/zx/bti.h>
#include <lib/zx/pmt.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <span>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>

namespace virtio_display {

// Implements generic parts of virtio14 over the PCI transport.
class VirtioPciDevice : public virtio::Device {
 public:
  static zx::result<std::unique_ptr<VirtioPciDevice>> Create(
      zx::bti bti, std::unique_ptr<virtio::Backend> backend);

  // `bti` is used to obtain physical memory addresses given to the virtio
  // device.
  //
  // `virtio_control_queue_buffer_pool` and `virtio_cursor_queue_buffer_pool`
  // must be large enough to store requests and responses exchanged with the
  // virtio device. The buffers must reside at
  // `virtio_control_queue_buffer_pool_physical_address` and
  // `virtio_cursor_queue_buffer_pool_physical_address`, respectively, in the
  // virtio device's physical address space.
  //
  // The instance hangs onto `virtio_control_queue_buffer_pool_vmo`,
  // `virtio_cursor_queue_buffer_pool_vmo`,
  // `virtio_control_queue_buffer_pool_pin`, and
  // `virtio_cursor_queue_buffer_pool_pin` for the duration of its lifetime.
  // They are intended to keep the memory backing
  // `virtio_control_queue_buffer_pool` and `virtio_cursor_queue_buffer_pool`
  // alive and pinned to `virtio_control_queue_buffer_pool_physical_address`
  // and `virtio_cursor_queue_buffer_pool_physical_address`, respectively.
  explicit VirtioPciDevice(zx::bti bti, std::unique_ptr<virtio::Backend> backend,
                           zx::vmo virtio_control_queue_buffer_pool_vmo,
                           zx::pmt virtio_control_queue_buffer_pool_pin,
                           zx_paddr_t virtio_control_queue_buffer_pool_physical_address,
                           std::span<uint8_t> virtio_control_queue_buffer_pool,
                           zx::vmo virtio_cursor_queue_buffer_pool_vmo,
                           zx::pmt virtio_cursor_queue_buffer_pool_pin,
                           zx_paddr_t virtio_cursor_queue_buffer_pool_physical_address,
                           std::span<uint8_t> virtio_cursor_queue_buffer_pool);
  ~VirtioPciDevice() override;

  size_t GetCapabilitySetLimit() const { return capability_set_limit_; }

  // The optional features negotiated with the virtio device.
  //
  // Must only be called after Init() succeeds.
  uint64_t Features() const { return features_; }

  // Synchronous request/response exchange on the main `controlq` virtqueue.
  void ExchangeControlqVariableLengthRequestResponse(
      std::span<const uint8_t> request, std::function<void(std::span<uint8_t>)> callback);

  // Templated request/response exchange on the main `control` virtqueue.
  //
  // The returned reference points to data owned by the VirtioPciDevice
  // instance, and is only valid until the method is called again.
  //
  // Call sites are expected to rely on partial template type inference. The
  // argument type should not be specified twice.
  //
  //     const virtio_abi::EmptyRequest request = {...};
  //     const auto& response =
  //         device->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(
  //             request);
  //
  // The current implementation fully serializes request/response exchanges.
  // More precisely, even if ExchangeControlqRequestResponse() is called concurrently,
  // the virtio device will never be given a request while another request is
  // pending. While this behavior is observable, it is not part of the API. The
  // implementation may be evolved to issue concurrent requests in the future.
  template <typename ResponseType, typename RequestType>
  const ResponseType& ExchangeControlqRequestResponse(const RequestType& request);

  // See ExchangeControlqRequestResponse() above; this method is functionally
  // the same, except it manages synchronous request/response exchange on the
  // `cursorq` virtqueue.
  template <typename ResponseType, typename RequestType>
  const ResponseType& ExchangeCursorqRequestResponse(const RequestType& request);

  // virtio::Device
  zx_status_t Init() override;
  void IrqRingUpdate() override;
  void IrqConfigChange() override;
  const char* tag() const override;

 private:
  // Called when the virtio device notifies the driver of having used a buffer
  // corresponding to the `controlq`.
  //
  // `chain_descriptor_index` is the virtqueue descriptor table index pointing to
  // the data that was consumed by the device. The indicated buffer, as well as
  // all the linked buffers, are now owned by the driver.
  // `chain_used_length` is "the number of bytes written into the device writable portion of
  // the buffer described by the descriptor chain".
  void VirtioControlqBufferUsedByDevice(uint32_t chain_descriptor_index,
                                        uint32_t chain_written_length)
      __TA_REQUIRES(virtio_control_queue_mutex_);

  // Called when the virtio device notifies the driver of having used a buffer
  // corresponding to the `cursorq`.
  //
  // `used_descriptor_index` is the virtqueue descriptor table index pointing to
  // the data that was consumed by the device. The indicated buffer, as well as
  // all the linked buffers, are now owned by the driver.
  void VirtioCursorqBufferUsedByDevice(uint32_t used_descriptor_index)
      __TA_REQUIRES(virtio_cursor_queue_mutex_);

  // Ensures that a single ExchangeControlqRequestResponse() call is in
  // progress at a time.
  fbl::Mutex exchange_controlq_request_response_mutex_;

  // Ensures that a single ExchangeCursorqRequestResponse() call is in
  // progress at a time.
  fbl::Mutex exchange_cursorq_request_response_mutex_;

  // Protects data members modified by multiple threads.
  fbl::Mutex virtio_control_queue_mutex_;
  fbl::Mutex virtio_cursor_queue_mutex_;

  // Signaled when the virtio device consumes a designated virtio queue
  // `controlq` buffer.
  //
  // The buffer is identified by `virtio_control_queue_request_index_`.
  fbl::ConditionVariable virtio_control_queue_buffer_used_signal_;

  // Signaled when the virtio device consumes a designated virtio queue
  // `cursorq` buffer.
  //
  // The buffer is identified by `virtio_cursor_queue_request_index_`.
  fbl::ConditionVariable virtio_cursor_queue_buffer_used_signal_;

  struct RequestResponse {
    uint16_t request_index;
    uint32_t written_length;
  };
  // nullopt iff no ExchangeControlqRequestResponse() is in progress.
  std::optional<RequestResponse> virtio_control_queue_request_response_
      __TA_GUARDED(virtio_control_queue_mutex_);

  // Identifies the request buffer allocated in ExchangeCursorqRequestResponse().
  //
  // nullopt iff no ExchangeCursorqRequestResponse() is in progress.
  std::optional<uint16_t> virtio_cursor_queue_request_index_
      __TA_GUARDED(virtio_cursor_queue_mutex_);

  // The GPU device's control virtqueue, or `controlq`.
  //
  // Defined in the virtio14 Section 5.7.2 "GPU Device" > "Virtqueues".
  virtio::Ring virtio_control_queue_ __TA_GUARDED(virtio_control_queue_mutex_);

  // The GPU device's cursor virtqueue, or `cursorq`.
  //
  // Defined in the virtio14 Section 5.7.2 "GPU Device" > "Virtqueues".
  virtio::Ring virtio_cursor_queue_ __TA_GUARDED(virtio_cursor_queue_mutex_);

  // Backs `virtio_control_queue_buffer_pool_`.
  const zx::vmo virtio_control_queue_buffer_pool_vmo_;

  // Backs `virtio_cursor_queue_buffer_pool_`.
  const zx::vmo virtio_cursor_queue_buffer_pool_vmo_;

  // Pins `virtio_control_queue_buffer_pool_vmo_` at a known physical address.
  const zx::pmt virtio_control_queue_buffer_pool_pin_;

  // Pins `virtio_cursor_queue_buffer_pool_vmo_` at a known physical address.
  const zx::pmt virtio_cursor_queue_buffer_pool_pin_;

  // The starting address of `virtio_control_queue_buffer_pool_`.
  const zx_paddr_t virtio_control_queue_buffer_pool_physical_address_;

  // The starting address of `virtio_cursor_queue_buffer_pool_`.
  const zx_paddr_t virtio_cursor_queue_buffer_pool_physical_address_;

  // Memory pinned at a known physical address, used for virtqueue buffers
  // correlating to the `controlq`.
  //
  // The span's data is modified by the driver and by the virtio device.
  const std::span<uint8_t> virtio_control_queue_buffer_pool_;

  // Memory pinned at a known physical address, used for virtqueue buffers
  // correlating to the `cursorq`.
  //
  // The span's data is modified by the driver and by the virtio device.
  const std::span<uint8_t> virtio_cursor_queue_buffer_pool_;

  // The number of capability sets supported by the virtio device.
  size_t capability_set_limit_ = {};

  // The optional features negotiated with the virtio device.
  uint64_t features_ = false;
};

template <typename ResponseType, typename RequestType>
const ResponseType& VirtioPciDevice::ExchangeControlqRequestResponse(const RequestType& request) {
  static constexpr size_t request_size = sizeof(RequestType);
  ResponseType* response_ptr = nullptr;

  auto callback = [&response_ptr](std::span<uint8_t> response_bytes) {
    static constexpr size_t response_size = sizeof(ResponseType);
    ZX_ASSERT(response_bytes.size() == response_size);
    response_ptr = reinterpret_cast<ResponseType*>(response_bytes.data());
  };

  ExchangeControlqVariableLengthRequestResponse(
      std::span(reinterpret_cast<const uint8_t*>(&request), request_size), std::move(callback));
  return *response_ptr;
}

template <typename ResponseType, typename RequestType>
const ResponseType& VirtioPciDevice::ExchangeCursorqRequestResponse(const RequestType& request) {
  static constexpr size_t request_size = sizeof(RequestType);
  static constexpr size_t response_size = sizeof(ResponseType);
  fdf::trace("Sending {}-byte request, expecting {}-byte response", request_size, response_size);

  // Request/response exchanges are fully serialized.
  //
  // Relaxing this implementation constraint would require the following:
  // * An allocation scheme for `virtio_cursor_queue_buffer_pool`. An easy
  //   solution is to sub-divide the area into equally-sized cells, where each
  //   cell can hold the largest possible request + response.
  // * A map of virtio queue descriptor index to condition variable, so multiple
  //   threads can be waiting on the device notification interrupt.
  // * Handling the case where `virtual_cursor_queue_buffer_pool` is full.
  //   Options are waiting on a new condition variable signaled whenever a
  //   buffer is released, and asserting that implies the buffer pool is
  //   statically sized to handle the maximum possible concurrency.
  //
  // Optionally, the locking around `virtio_cursor_queue_mutex_` could be more
  // fine-grained. Allocating the descriptors needs to be serialized, but
  // populating them can be done concurrently. Last, submitting the descriptors
  // to the virtio device must be serialized.
  fbl::AutoLock exhange_cursorq_request_response_lock(&exchange_cursorq_request_response_mutex_);

  fbl::AutoLock virtio_cursor_queue_lock(&virtio_cursor_queue_mutex_);

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
      virtio_cursor_queue_.AllocDescChain(/*count=*/2, &request_descriptor_index);
  ZX_ASSERT(request_descriptor);
  virtio_cursor_queue_request_index_ = request_descriptor_index;

  std::span<uint8_t> request_span = virtio_cursor_queue_buffer_pool_.subspan(0, request_size);
  std::memcpy(request_span.data(), &request, request_size);

  const zx_paddr_t request_physical_address = virtio_cursor_queue_buffer_pool_physical_address_;
  request_descriptor->addr = request_physical_address;
  static_assert(request_size <= std::numeric_limits<uint32_t>::max());
  request_descriptor->len = static_cast<uint32_t>(request_size);
  request_descriptor->flags = VRING_DESC_F_NEXT;

  vring_desc* const response_descriptor =
      virtio_cursor_queue_.DescFromIndex(request_descriptor->next);
  ZX_ASSERT(response_descriptor);

  std::span<uint8_t> response_span =
      virtio_cursor_queue_buffer_pool_.subspan(request_size, response_size);
  std::fill(response_span.begin(), response_span.end(), 0);

  const zx_paddr_t response_physical_address = request_physical_address + request_size;
  response_descriptor->addr = response_physical_address;
  static_assert(response_size <= std::numeric_limits<uint32_t>::max());
  response_descriptor->len = static_cast<uint32_t>(response_size);
  response_descriptor->flags = VRING_DESC_F_WRITE;

  // Submit the transfer & wait for the response
  virtio_cursor_queue_.SubmitChain(request_descriptor_index);
  virtio_cursor_queue_.Kick();

  virtio_cursor_queue_buffer_used_signal_.Wait(&virtio_cursor_queue_mutex_);

  return *reinterpret_cast<ResponseType*>(response_span.data());
}

}  // namespace virtio_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_VIRTIO_PCI_DEVICE_H_
