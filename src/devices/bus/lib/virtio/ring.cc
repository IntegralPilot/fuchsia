// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <inttypes.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/stdcompat/bit.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>
#include <lib/zx/vmar.h>
#include <limits.h>
#include <stdint.h>
#include <string.h>

#include <fbl/algorithm.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace virtio {

void virtio_dump_desc(const struct vring_desc* desc) {
  printf("vring descriptor %p: ", desc);
  printf("[addr=%#" PRIx64 ", ", desc->addr);
  printf("len=%d, ", desc->len);
  printf("flags=%#04hx, ", desc->flags);
  printf("next=%#04hx]\n", desc->next);
}

Ring::Ring(Device* device) : device_(device) {}

Ring::Ring(Ring&& other) noexcept {
  index_ = other.index_;
  device_ = other.device_;
  other.device_ = nullptr;
  ring_buffer_ = std::move(other.ring_buffer_);
  ring_ = other.ring_;
  other.ring_ = vring{};
}

Ring::~Ring() = default;

Ring& Ring::operator=(Ring&& other) noexcept {
  index_ = other.index_;
  device_ = other.device_;
  other.device_ = nullptr;
  ring_buffer_ = std::move(other.ring_buffer_);
  ring_ = other.ring_;
  other.ring_ = vring{};
  return *this;
}

zx_status_t Ring::Init(uint16_t index) {
  return Init(/*index=*/index, /*count=*/device_->GetRingSize(index));
}

zx_status_t Ring::Init(uint16_t index, uint16_t count) {
  zxlogf(TRACE, "%s: index %u, count %u", __func__, index, count);

  // check that count is a power of 2
  if (!cpp20::has_single_bit(count)) {
    zxlogf(ERROR, "ring count: %u is not a power of 2", count);
    return ZX_ERR_INVALID_ARGS;
  }

  index_ = index;

  // make sure the count is available in this ring
  uint16_t max_ring_size = device_->GetRingSize(index);
  if (count > max_ring_size) {
    zxlogf(ERROR, "ring init count too big for hardware %u > %u", count, max_ring_size);
    return ZX_ERR_OUT_OF_RANGE;
  }

  // allocate a ring
  const size_t vring_required_size = vring_size(count, zx_system_get_page_size());

  // DMA buffer size must be multiples of page size. Round up to the nearest
  // page size.
  const size_t dma_buffer_size = fbl::round_up(vring_required_size, zx_system_get_page_size());
  zxlogf(TRACE, "%s: need %zu bytes", __func__, dma_buffer_size);

  std::unique_ptr<dma_buffer::BufferFactory> buffer_factory = dma_buffer::CreateBufferFactory();
  zx_status_t status =
      buffer_factory->CreateContiguous(device_->bti(), dma_buffer_size, /*alignment_log2=*/0,
                                       dma_buffer::CacheOptions::kEnabled, &ring_buffer_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to allocate ring buffer of size %zu: %s", dma_buffer_size,
           zx_status_get_string(status));
    return status;
  }

  zxlogf(TRACE, "%s: allocated vring at %p, physical address %#" PRIxPTR, __func__,
         ring_buffer_->virt(), ring_buffer_->phys());

  /* initialize the ring */
  vring_init(&ring_, count, ring_buffer_->virt(), zx_system_get_page_size());
  ring_.free_list = 0xffff;
  ring_.free_count = 0;

  /* add all the descriptors to the free list */
  for (uint16_t i = 0; i < count; i++) {
    FreeDesc(i);
  }

  /* register the ring with the device */
  zx_paddr_t pa_desc = ring_buffer_->phys();
  zx_paddr_t pa_avail = pa_desc + ((uintptr_t)ring_.avail - (uintptr_t)ring_.desc);
  zx_paddr_t pa_used = pa_desc + ((uintptr_t)ring_.used - (uintptr_t)ring_.desc);
  device_->SetRing(index_, count, pa_desc, pa_avail, pa_used);

  return ZX_OK;
}

void Ring::FreeDesc(uint16_t desc_index) {
  zxlogf(TRACE, "%s: index %u free_count %u", __func__, desc_index, ring_.free_count);
  if (!IsValidDescIndex(desc_index)) {
    zxlogf(ERROR, "%s: index %u out of bounds (ring size %u)", __func__, desc_index, ring_.num);
    return;
  }
  ring_.desc[desc_index].next = ring_.free_list;
  ring_.free_list = desc_index;
  ring_.free_count++;
}

struct vring_desc* Ring::AllocDescChain(uint16_t count, uint16_t* start_index) {
  if (count == 0 || ring_.free_count < count) {
    return nullptr;
  }

  uint16_t head_index = ring_.free_list;
  uint16_t curr_index = head_index;

  // Mark all but the last descriptor as part of the sequence.
  for (uint16_t i = 0; i < count - 1; i++) {
    assert(curr_index < ring_.num);
    ring_.desc[curr_index].flags |= VRING_DESC_F_NEXT;
    curr_index = ring_.desc[curr_index].next;
  }
  assert(curr_index < ring_.num);

  ring_.free_list = ring_.desc[curr_index].next;
  // Clear the flag to signal the end of the sequence.
  ring_.desc[curr_index].flags &= static_cast<uint16_t>(~VRING_DESC_F_NEXT);
  ring_.desc[curr_index].next = 0;
  ring_.free_count -= count;

  if (start_index) {
    *start_index = head_index;
  }

  return &ring_.desc[head_index];
}

void Ring::SubmitChain(uint16_t desc_index) {
  zxlogf(TRACE, "%s: desc %u", __func__, desc_index);
  if (!IsValidDescIndex(desc_index)) {
    zxlogf(ERROR, "%s: index %u out of bounds (ring size %u)", __func__, desc_index, ring_.num);
    return;
  }

  /* add the chain to the available list */
  struct vring_avail* avail = ring_.avail;

  avail->ring[avail->idx & ring_.num_mask] = desc_index;
  // Write memory barrier before updating avail->idx; updates to the descriptor ring must be
  // visible before an updated avail->idx.
  hw_wmb();
  avail->idx++;
}

void Ring::Kick() {
  zxlogf(TRACE, "%s: entry", __func__);
  // Write memory barrier before notifying the device. Updates to avail->idx must be visible
  // before the device sees the wakeup notification (so it processes the latest descriptors).
  hw_mb();

  device_->RingKick(index_);
}

}  // namespace virtio
