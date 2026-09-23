// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_SPINEL_FRAMER_SPINEL_FRAMER_H_
#define SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_SPINEL_FRAMER_SPINEL_FRAMER_H_

#include <fidl/fuchsia.hardware.spi/cpp/wire.h>
#include <zircon/types.h>

#include "spinel_inspector.h"

namespace ot {

class SpinelFramer {
 public:
  SpinelFramer() {}
  void Init(fidl::WireSyncClient<fuchsia_hardware_spi::Device>* spi,
            uint16_t spi_rx_align_allowance = 0);
  void HandleInterrupt();
  uint32_t GetTimeoutMs(void);
  zx_status_t TryBufferTxPacket(uint8_t* packet, uint16_t length);
  void ReceivePacketFromRadio(uint8_t* rxPacket, uint16_t* length);
  bool IsPacketPresent(void);
  void TrySpiTransaction(void);
  void SetInboundAllowanceStatus(bool status);
  ::zx::vmo InspectorDuplicateVmo() const;

 private:
  static constexpr uint16_t kMaxFrameSize = 2048;
  static constexpr uint8_t kHeaderLen = 5;
  static constexpr uint8_t kHeaderResetFlag = 0x80;
  static constexpr uint8_t kHeaderPatternValue = 0x02;
  static constexpr uint8_t kHeaderPatternMask = 0x03;
  static constexpr uint8_t kSpiRxAllignAllowanceMax = 16;
  static constexpr uint8_t kDebugBytesPerLine = 16;

  fidl::WireSyncClient<fuchsia_hardware_spi::Device>* spi_;
  bool interrupt_fired_ = false;
  // The SPI RX frame length to be received.
  // If the length is 0, no RX frame is pending receiving.
  uint16_t spi_rx_payload_size_ = 0;
  uint8_t spi_rx_frame_buffer_[kMaxFrameSize + kSpiRxAllignAllowanceMax];
  // The SPI TX frame length to be sent out.
  uint16_t spi_tx_payload_size_ = 0;
  uint8_t spi_tx_frame_buffer_[kMaxFrameSize + kSpiRxAllignAllowanceMax];
  // There is a frame in `spi_tx_frame_buffer_` and ready to be sent out.
  bool spi_tx_is_ready_ = false;
  int spi_tx_refused_count_ = 0;
  bool radio_data_rx_pending_ = false;
  uint16_t spi_rx_align_allowance_ = 0;
  uint8_t spi_small_packet_size_ = 32;  // in bytes
  bool slave_did_reset_ = false;
  bool dump_stats_ = false;

  uint64_t slave_reset_count_ = 0;
  uint64_t spi_frame_count_ = 0;
  uint64_t spi_valid_frame_count_ = 0;
  uint64_t spi_garbage_frame_count_ = 0;
  uint64_t spi_duplex_frame_count_ = 0;
  uint64_t spi_unresponsive_frame_count_ = 0;
  uint64_t rx_frame_byte_count_ = 0;
  uint64_t tx_frame_byte_count_ = 0;
  uint64_t rx_frame_count_ = 0;
  uint64_t tx_frame_count_ = 0;

  zx_status_t PushPullSpi(void);
  void ClearStats(void);
  void LogDebugBuffer(const char* desc, const uint8_t* buffer_ptr, uint16_t buffer_len);
  uint8_t* GetRealRxFrameStart(void);
  zx_status_t DoSpiXfer(uint16_t len);
  bool CheckAndClearInterrupt(void);
  void DebugSpiHeader(const char* hint);
  bool has_inbound_allowance_ = false;
  bool did_print_rate_limit_log_ = false;
  SpinelFramerInspector inspector_;
};

}  // namespace ot

#endif  // SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_SPINEL_FRAMER_SPINEL_FRAMER_H_
