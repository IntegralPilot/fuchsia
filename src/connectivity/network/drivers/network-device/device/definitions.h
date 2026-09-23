// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_DEFINITIONS_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_DEFINITIONS_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <zircon/types.h>

#include <array>

#include <fbl/intrusive_double_list.h>

#include "src/lib/vmo_store/vmo_store.h"

namespace network {
namespace netdev = fuchsia_hardware_network;
namespace netdriver = fuchsia_hardware_network_driver;
constexpr uint16_t kMaxFifoDepth = ZX_FIFO_MAX_SIZE_BYTES / sizeof(uint16_t);

namespace internal {
template <typename T>
using BufferParts = std::array<T, netdriver::kMaxBufferParts>;
using netdev::wire::VmoId;

constexpr uint32_t kInvalidIdx = std::numeric_limits<uint32_t>::max();

struct DefaultVmoNodeTag {};
struct RxVmoNodeTag {};

struct DataVmoMeta;
using DataVmoList = fbl::TaggedDoublyLinkedList<DataVmoMeta*, DefaultVmoNodeTag>;
using RxVmoList = fbl::TaggedDoublyLinkedList<DataVmoMeta*, RxVmoNodeTag>;

enum class VmoState : uint8_t {
  kUnprepared,
  kPreparing,
  kPrepared,
  kReleasing,
};

struct DataVmoMeta : public fbl::ContainableBaseClasses<
                         fbl::TaggedDoublyLinkedListable<DataVmoMeta*, DefaultVmoNodeTag,
                                                         fbl::NodeOptions::AllowMove>,
                         fbl::TaggedDoublyLinkedListable<DataVmoMeta*, RxVmoNodeTag,
                                                         fbl::NodeOptions::AllowMove>> {
  const VmoId id;
  const uint16_t num_rx_buffers;
  bool tx_registered{false};
  VmoState state{VmoState::kUnprepared};
  // Callback invoked when this VMO (as the last in a batch) completes preparation or release.
  // control_lock_ is not held when the callback is called.
  fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> batch_completion;

  // RxQueue runtime state, rx_lock_ needs to be locked when accessing the following fields.

  // How many buffers are being withheld. Withheld buffers are buffers that are sent by client,
  // but not eligible for pushing as the device impl's Rx space buffer. These buffers are in
  // |in_flight_|, but not in |available_queue_|. There are 2 reasons for this to happen:
  //   1) The VMO for the buffer has not been prepared, so we cannot send it.
  //   2) We've decided to release the target VMO, the buffers from that VMO are withheld to allow
  //      progress on the release.
  uint32_t withheld_rx_buffers{0};
  // The index of the head of an in-flight Rx buffer. The buffers from the same VMO are chained
  // through |RxQueue::InFlightBuffer::next_withheld_inflight_buffer|.
  uint32_t head_withheld_rx_buffers{kInvalidIdx};
};
using DataVmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint8_t, DataVmoMeta>>;
}  // namespace internal

}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_DEFINITIONS_H_
