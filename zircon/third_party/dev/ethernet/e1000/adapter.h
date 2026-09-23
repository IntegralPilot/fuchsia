/*-
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * Copyright (c) 2016 Nicole Graziano <nicole@nextbsd.org>
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */

#ifndef ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_ADAPTER_H_
#define ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_ADAPTER_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <lib/ddk/io-buffer.h>
#include <lib/stdcompat/bit.h>
#include <net/ethernet.h>

#include "e1000_api.h"

namespace e1000 {

constexpr size_t kTxDepth = 256;
constexpr size_t kRxDepth = 256;

// Determine the maximum number of buffers per call to CompleteRx and CompleteTx. These are limited
// by either the RX/TX depth or FIDL protocol maximum, whichever is lowest.
constexpr size_t kRxBuffersPerBatch =
    std::min<size_t>(fuchsia_hardware_network_driver::wire::kMaxRxBuffers, kRxDepth);
constexpr size_t kTxResultsPerBatch =
    std::min<size_t>(fuchsia_hardware_network_driver::wire::kMaxTxResults, kTxDepth);

constexpr uint32_t kEthMtu = 1500;
constexpr uint32_t kMaxFrameSize = kEthMtu + ETHER_HDR_LEN + ETHER_CRC_LEN;

constexpr uint32_t kMaxBufferLength = 2048;
constexpr uint32_t kBufferAlignment = 2048;

// The closest power of two that exceeds the max frame size.
constexpr uint32_t kMinRxBufferLength = cpp20::bit_ceil(kMaxFrameSize);
// Minimum ethernet frame size (minus frame check sequence)
constexpr uint32_t kMinTxBufferLength = ETH_ZLEN;

struct adapter {
  struct e1000_hw hw;
  struct e1000_osdep osdep;
  zx::interrupt irq;
  zx::bti bti;
  io_buffer_t buffer;

  uint32_t dmac;
  // Indicates that the adapter supports AMT, active management technology, this determines when the
  // driver should acquire control over the hardware and not.
  bool has_amt;
  // Indicates that the adapter has a management interface and that the driver must leave the
  // interface enabled to allow management frames through.
  bool has_manage;

  uint32_t flags;

  // base physical addresses for
  // tx/rx rings and rx buffers
  // store as 64bit integer to match hw register size
  uint64_t txd_phys;
  uint64_t rxd_phys;

  bool was_reset;

  std::mutex tx_mutex;
  std::mutex rx_mutex;

  // callback interface to attached network device layer
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> ifc;

  std::optional<fdf::MmioBuffer> bar0_mmio;
  std::optional<fdf::MmioBuffer> flash_mmio;

  fuchsia_hardware_pci::InterruptMode irq_mode;

  // Indicate a size for the arena so that we avoid re-allocation as much as possible. Don't use
  // this arena for anything other than these RX buffers.
  static constexpr size_t kRxArenaSize = fidl::MaxSizeInChannel<
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest,
      fidl::MessageDirection::kSending>();
  fidl::Arena<kRxArenaSize> rx_buffer_arena;

  // Indicate a size for the arena so that we avoid re-allocation as much as possible. Don't use
  // this arena for anything other than these TX results.
  static constexpr size_t kTxArenaSize = fidl::MaxSizeInChannel<
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest,
      fidl::MessageDirection::kSending>();
  fidl::Arena<kTxArenaSize> tx_buffer_arena;

  fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer> rx_buffers{rx_buffer_arena,
                                                                               kRxBuffersPerBatch};
  fidl::VectorView<fuchsia_hardware_network_driver::wire::TxResult> tx_results{rx_buffer_arena,
                                                                               kTxResultsPerBatch};
};

// Ensure that adapter is an aggregate type so that it is subject to aggregate initialization. If
// this is not the case then a lot of values may potentially be left uninitialized which will break
// things. Aggregate initialization basically means that everything is zero'd out, which is what a
// lot of the third party code expects. For a type to be aggregate it cannot have user-declared
// or inherited constructors, no private or protected non-static data members, no virtual, private
// or protected base classes, no virtual member functions, no default member initializers. This is
// not necessarily an exhaustive list, but the ones you are most likely to run into are default
// member initializers and constructors. If you need to add those make sure that everything else is
// correctly initialized as well.
static_assert(std::is_aggregate_v<adapter>, "adapter must be an aggregate type");

}  // namespace e1000

#endif  // ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_ADAPTER_H_
