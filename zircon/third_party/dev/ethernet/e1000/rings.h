// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_RINGS_H_
#define ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_RINGS_H_

#include <lib/stdcompat/bit.h>

#include <array>

#include <fbl/macros.h>

#include "e1000_api.h"

namespace e1000 {

template <uint32_t Depth>
class TxRing {
  static_assert(cpp20::has_single_bit(Depth), "Depth must be a power of two");

 public:
  TxRing() = default;
  DISALLOW_COPY_ASSIGN_AND_MOVE(TxRing);

  void AssignDescriptorMmio(void* descriptor_mmio) {
    descriptors_ = static_cast<e1000_tx_desc*>(descriptor_mmio);
  }

  // Push TX buffer to the ring. Returns the NEW tail after pushing to match the behavior of the
  // hardware TX ring. Note that this is different from the RX ring.
  uint32_t Push(uint32_t id, zx_paddr_t physical_addr, uint64_t length) {
    ids_[tail_] = id;
    descriptors_[tail_].buffer_addr = physical_addr;
    descriptors_[tail_].lower.data =
        static_cast<uint32_t>(E1000_TXD_CMD_EOP | E1000_TXD_CMD_IFCS | E1000_TXD_CMD_RS | length);
    tail_ = (tail_ + 1) & (Depth - 1);
    ++size_;
    return tail_;
  }

  void Pop() {
    descriptors_[head_].upper.data = 0;
    head_ = (head_ + 1) & (Depth - 1);
    --size_;
  }

  void Clear() {
    size_ = head_ = tail_ = 0;
    memset(descriptors_, 0, sizeof(e1000_tx_desc) * Depth);
    ids_.fill({});
  }

  uint32_t HeadIndex() const { return head_; }
  uint32_t HeadId() const { return ids_[head_]; }
  e1000_tx_desc& HeadDesc() { return descriptors_[head_]; }
  bool IsEmpty() const { return size_ == 0; }
  uint32_t Size() const { return size_; }
  bool Available() const { return descriptors_[head_].upper.fields.status & E1000_TXD_STAT_DD; }

  uint32_t Id(size_t index) const { return ids_[index]; }
  e1000_tx_desc& Desc(size_t index) { return descriptors_[index]; }

 private:
  e1000_tx_desc* descriptors_ = nullptr;
  std::array<uint32_t, Depth> ids_;
  uint32_t head_ = 0;
  uint32_t tail_ = 0;
  uint32_t size_ = 0;
};

template <uint32_t Depth, typename Descriptor>
class RxRing {
  static_assert(cpp20::has_single_bit(Depth), "Depth must be a power of two");
  static_assert(std::is_same_v<Descriptor, e1000_adv_rx_desc> ||
                    std::is_same_v<Descriptor, e1000_rx_desc_extended> ||
                    std::is_same_v<Descriptor, e1000_rx_desc>,
                "Descriptor type must match one of the supported types");

 public:
  RxRing() = default;
  DISALLOW_COPY_ASSIGN_AND_MOVE(RxRing);

  void AssignDescriptorMmio(void* descriptor_mmio) {
    descriptors_ = static_cast<Descriptor*>(descriptor_mmio);
  }

  // Push RX space to the ring. Returns the PREVIOUS tail before pushing to match the expectation
  // of the hardware RX ring. Note that this is different from the TX ring.
  uint32_t Push(uint32_t id, zx_paddr_t physical_addr) {
    ids_[tail_] = id;
    if constexpr (std::is_same_v<Descriptor, e1000_adv_rx_desc>) {
      descriptors_[tail_].read.pkt_addr = physical_addr;
    } else if constexpr (std::is_same_v<Descriptor, e1000_rx_desc_extended>) {
      descriptors_[tail_].read.buffer_addr = physical_addr;
      descriptors_[tail_].wb.upper.status_error = 0;
    } else if constexpr (std::is_same_v<Descriptor, e1000_rx_desc>) {
      descriptors_[tail_].buffer_addr = physical_addr;
      descriptors_[tail_].status = 0;
    }
    const uint32_t prev_tail = tail_;
    tail_ = (tail_ + 1) & (Depth - 1);
    ++size_;
    return prev_tail;
  }

  void Pop() {
    if constexpr (std::is_same_v<Descriptor, e1000_adv_rx_desc>) {
      descriptors_[head_].wb.upper.status_error = 0;
    } else if constexpr (std::is_same_v<Descriptor, e1000_rx_desc_extended>) {
      descriptors_[head_].wb.upper.status_error &= 0xFF;
    } else if constexpr (std::is_same_v<Descriptor, e1000_rx_desc>) {
      descriptors_[head_].status = 0;
    }
    head_ = (head_ + 1) & (Depth - 1);
    --size_;
  }

  void Clear() {
    size_ = head_ = tail_ = 0;
    memset(descriptors_, 0, sizeof(e1000_tx_desc) * Depth);
    ids_.fill({});
  }

  uint32_t HeadId() const { return ids_[head_]; }
  bool IsEmpty() const { return size_ == 0; }
  uint32_t Size() const { return size_; }
  bool Available(uint16_t* out_len) const {
    if (IsEmpty()) {
      return false;
    }
    uint32_t status = 0;
    if constexpr (std::is_same_v<Descriptor, e1000_adv_rx_desc> ||
                  std::is_same_v<Descriptor, e1000_rx_desc_extended>) {
      status = descriptors_[head_].wb.upper.status_error;
      if (unlikely(status & E1000_RXDEXT_ERR_FRAME_ERR_MASK)) {
        *out_len = 0;
      } else {
        *out_len = descriptors_[head_].wb.upper.length;
      }
    } else if constexpr (std::is_same_v<Descriptor, e1000_rx_desc>) {
      status = descriptors_[head_].status;
      if (unlikely(status & E1000_RXD_ERR_FRAME_ERR_MASK)) {
        *out_len = 0;
      } else {
        *out_len = descriptors_[head_].length;
      }
    }
    return status & E1000_TXD_STAT_DD;
  }

  uint32_t Id(size_t index) const { return ids_[index]; }
  Descriptor& Desc(size_t index) { return descriptors_[index]; }

 private:
  Descriptor* descriptors_ = nullptr;
  std::array<uint32_t, Depth> ids_;
  uint32_t head_ = 0;
  uint32_t tail_ = 0;
  uint32_t size_ = 0;
};

}  // namespace e1000

#endif  // ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_RINGS_H_
