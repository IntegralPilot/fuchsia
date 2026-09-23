// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_RING_H_
#define SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_RING_H_

#include <lib/dma-buffer/buffer.h>
#include <lib/virtio/internal/barriers.h>
#include <zircon/types.h>

#include <memory>

#include <virtio/virtio_ring.h>

namespace virtio {

class Device;

class Ring {
 public:
  explicit Ring(Device* device);

  Ring(const Ring& other) = delete;
  Ring& operator=(const Ring& other) = delete;
  Ring(Ring&& other) noexcept;
  Ring& operator=(Ring&& other) noexcept;

  ~Ring();

  // Initialize ring |index| with default (device-offered) size.
  zx_status_t Init(uint16_t index);
  zx_status_t Init(uint16_t index, uint16_t count);

  void FreeDesc(uint16_t desc_index);
  struct vring_desc* AllocDescChain(uint16_t count, uint16_t* start_index);
  void SubmitChain(uint16_t desc_index);
  void Kick();

  bool IsValidDescIndex(uint16_t index) const { return index < ring_.num; }

  struct vring_desc* DescFromIndex(uint16_t index) {
    if (!IsValidDescIndex(index)) {
      return nullptr;
    }
    return &ring_.desc[index];
  }

  template <typename T>
  void IrqRingUpdate(T free_chain);

  bool NoNotify() { return ring_.used->flags & VRING_USED_F_NO_NOTIFY; }
  void SetNoInterrupt() { ring_.avail->flags |= VRING_AVAIL_F_NO_INTERRUPT; }
  // Note that unless you are performing you own memory barriers, you most likely want to use the
  // combined ClearNoInterruptCheckHasWork below.
  void ClearNoInterrupt() { ring_.avail->flags &= ~VRING_AVAIL_F_NO_INTERRUPT; }
  bool HasWork() { return ring_.used->idx != ring_.last_used; }

  // Re-enables interrupts and then checks if there is any work pending. This is performed with an
  // appropriate barrier to ensure the driver cannot observe a state that would cause it to fail to
  // an inject an interrupt, whilst at the same time we are waiting for one.
  bool ClearNoInterruptCheckHasWork() {
    ClearNoInterrupt();
    // Perform a memory barrier to ensure the load in HasWork does not get reordered before the
    // store in ClearNoInterrupt.
    hw_mb();
    return HasWork();
  }

  // Provides access to the underlying memory. Meant for use in tests.
  vring& vring_unsafe() { return ring_; }

 private:
  Device* device_ = nullptr;

  std::unique_ptr<dma_buffer::ContiguousBuffer> ring_buffer_;

  uint16_t index_ = 0;

  vring ring_ = {};
};

// perform the main loop of finding free descriptor chains and passing it to a passed in function
template <typename T>
inline void Ring::IrqRingUpdate(T free_chain) {
  // TODO(https://fxbug.dev/349298661): Add back the trace logs below, once the
  // library only supports DFv2 logging (FDF_LOG).
  //
  // TRACEF("used flags %#x idx %#x last_used %u", ring_.used->flags, ring_.used->idx,
  //        ring_.last_used);

  // find a new free chain of descriptors
  uint16_t cur_idx = ring_.used->idx;
  uint16_t i = ring_.last_used;
  // Read memory barrier before processing a descriptor chain. If we see an updated used->idx
  // we must see updated descriptor chains in the used ring.
  hw_rmb();
  for (; i != cur_idx; ++i) {
    // TRACEF("looking at idx %u\n", i);

    struct vring_used_elem* used_elem = &ring_.used->ring[i & ring_.num_mask];
    // TRACEF("used chain id %u, len %u\n", used_elem->id, used_elem->len);

    // free the chain
    free_chain(used_elem);
  }
  ring_.last_used = i;
}

void virtio_dump_desc(const struct vring_desc* desc);

}  // namespace virtio

#endif  // SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_RING_H_
