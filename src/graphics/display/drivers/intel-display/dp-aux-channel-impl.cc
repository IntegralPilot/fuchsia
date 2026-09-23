// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/dp-aux-channel-impl.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cstdint>
#include <span>

#include <fbl/auto_lock.h>

#include "src/graphics/display/drivers/intel-display/ddi-aux-channel.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace intel_display {
namespace {

// Aux port functions

// 4-bit request type in Aux channel request messages.
enum {
  DP_REQUEST_I2C_WRITE = 0,
  DP_REQUEST_I2C_READ = 1,
  DP_REQUEST_NATIVE_WRITE = 8,
  DP_REQUEST_NATIVE_READ = 9,
};

// 4-bit statuses in Aux channel reply messages.
enum {
  DP_REPLY_AUX_ACK = 0,
  DP_REPLY_AUX_NACK = 1,
  DP_REPLY_AUX_DEFER = 2,
  DP_REPLY_I2C_NACK = 4,
  DP_REPLY_I2C_DEFER = 8,
};

// The I2C address for writing the DDC segment.
//
// VESA Enhanced Display Data Channel (E-DDC) Standard version 1.3 revised
// Dec 31 2020, Section 2.2.3 "DDC Addresses", page 17.
constexpr uint8_t kDdcSegmentI2cTargetAddress = 0x30;

// The I2C address for writing the DDC data offset/reading DDC data.
//
// VESA Enhanced Display Data Channel (E-DDC) Standard version 1.3 revised
// Dec 31 2020, Section 2.2.3 "DDC Addresses", page 17.
constexpr uint8_t kDdcDataI2cTargetAddress = 0x50;

}  // namespace

zx::result<DdiAuxChannel::ReplyInfo> DpAuxChannelImpl::DoTransaction(
    const DdiAuxChannel::Request& request, std::span<uint8_t> reply_data_buffer) {
  // If the DisplayPort sink device isn't ready to handle an Aux message,
  // it can return an AUX_DEFER reply, which means we should retry the
  // request. The spec added a requirement for >=7 defer retries in v1.3,
  // but there are no requirements before that nor is there a max value. 16
  // retries is pretty arbitrary and might need to be increased for slower
  // displays.
  const int kMaxDefers = 16;

  // Per table 2-43 in v1.1a, we need to retry >3 times, since some
  // DisplayPort sink devices time out on the first DP aux request
  // but succeed on later requests.
  const int kMaxTimeouts = 5;

  unsigned defers_seen = 0;
  unsigned timeouts_seen = 0;

  for (;;) {
    zx::result<DdiAuxChannel::ReplyInfo> transaction_result =
        aux_channel_.DoTransaction(request, reply_data_buffer);
    if (transaction_result.is_error()) {
      if (transaction_result.error_value() == ZX_ERR_IO_MISSED_DEADLINE) {
        if (++timeouts_seen == kMaxTimeouts) {
          fdf::debug("DP aux: Got too many timeouts ({})", kMaxTimeouts);
          return transaction_result;
        }
        // Retry on timeout.
        continue;
      }

      // We do not retry if sending the raw message failed for
      // an unexpected reason.
      return transaction_result;
    }

    uint8_t header_byte = transaction_result->reply_header;
    uint8_t padding = header_byte & 0xf;
    uint8_t status = static_cast<uint8_t>(header_byte >> 4);
    // Sanity check: The padding should be zero.  If it's not, we
    // shouldn't return an error, in case this space gets used for some
    // later extension to the protocol.  But report it, in case this
    // indicates some problem.
    if (padding) {
      fdf::info("DP aux: Reply header padding is non-zero (header byte: 0x{:x})", header_byte);
    }

    switch (status) {
      case DP_REPLY_AUX_ACK:
        // The AUX_ACK implies that we got an I2C ACK too.
        return transaction_result;
      case DP_REPLY_AUX_NACK:
        fdf::trace("DP aux: Reply was not an ack (got AUX_NACK)");
        return zx::error_result(ZX_ERR_IO_REFUSED);
      case DP_REPLY_AUX_DEFER:
        if (++defers_seen == kMaxDefers) {
          fdf::trace("DP aux: Received too many AUX DEFERs ({})", kMaxDefers);
          return zx::error_result(ZX_ERR_IO_MISSED_DEADLINE);
        }
        // Go around the loop again to retry.
        continue;
      case DP_REPLY_I2C_NACK:
        fdf::trace("DP aux: Reply was not an ack (got I2C_NACK)");
        return zx::error_result(ZX_ERR_IO_REFUSED);
      case DP_REPLY_I2C_DEFER:
        // TODO(https://fxbug.dev/42106274): Implement handling of I2C_DEFER.
        fdf::trace("DP aux: Received I2C_DEFER (not implemented)");
        return zx::error_result(ZX_ERR_NEXT);
      default:
        fdf::trace("DP aux: Unrecognized reply (header byte: 0x{:x})", header_byte);
        return zx::error_result(ZX_ERR_IO_DATA_INTEGRITY);
    }
  }
}

zx_status_t DpAuxChannelImpl::DpAuxRead(uint32_t dp_cmd, uint32_t addr, uint8_t* buf, size_t size) {
  while (size > 0) {
    uint32_t chunk_size = static_cast<uint32_t>(std::min<size_t>(size, DdiAuxChannel::kMaxOpSize));
    size_t bytes_read = 0;
    zx_status_t status = DpAuxReadChunk(dp_cmd, addr, buf, chunk_size, &bytes_read);
    if (status != ZX_OK) {
      return status;
    }
    if (bytes_read == 0) {
      // We failed to make progress on the last call.  To avoid the
      // risk of getting an infinite loop from that happening
      // continually, we return.
      return ZX_ERR_IO;
    }
    buf += bytes_read;
    size -= bytes_read;
  }
  return ZX_OK;
}

zx_status_t DpAuxChannelImpl::DpAuxReadChunk(uint32_t dp_cmd, uint32_t addr, uint8_t* buf,
                                             uint32_t size_in, size_t* size_out) {
  const DdiAuxChannel::Request request = {
      .address = static_cast<int32_t>(addr),
      .command = static_cast<int8_t>(dp_cmd),
      .op_size = static_cast<int8_t>(size_in),
      .data = std::span<uint8_t>(),
  };

  zx::result<DdiAuxChannel::ReplyInfo> result =
      DoTransaction(request, std::span<uint8_t>(buf, size_in));
  if (result.is_error()) {
    return result.error_value();
  }

  // The cast is not UB because `reply_data_size` is guaranteed to be between
  // 1 and 16.
  const size_t bytes_read = static_cast<size_t>(result.value().reply_data_size);
  if (static_cast<size_t>(bytes_read) > size_in) {
    fdf::warn("DP aux read: Reply was larger than requested");
    return ZX_ERR_IO;
  }
  *size_out = bytes_read;
  return ZX_OK;
}

zx_status_t DpAuxChannelImpl::DpAuxWrite(uint32_t dp_cmd, uint32_t addr, const uint8_t* buf,
                                         size_t size) {
  // Implement this if it's ever needed
  ZX_ASSERT_MSG(size <= 16, "message too large");

  const DdiAuxChannel::Request request = {
      .address = static_cast<int32_t>(addr),
      .command = static_cast<int8_t>(dp_cmd),
      .op_size = static_cast<int8_t>(size),
      .data = std::span<const uint8_t>(buf, size),
  };

  // In case of a short write, receives the amount of written bytes.
  uint8_t reply_data[1];

  zx::result<DdiAuxChannel::ReplyInfo> transaction_result = DoTransaction(request, reply_data);
  if (transaction_result.is_error()) {
    return transaction_result.error_value();
  }
  // TODO(https://fxbug.dev/42106274): Handle the case where the hardware did a short write,
  // for which we could send the remaining bytes.
  if (transaction_result->reply_data_size != 0) {
    fdf::warn("DP aux write: Unexpected reply size");
    return ZX_ERR_IO;
  }
  return ZX_OK;
}

zx::result<> DpAuxChannelImpl::ReadEdidBlock(int index,
                                             std::span<uint8_t, edid::kBlockSize> edid_block) {
  ZX_DEBUG_ASSERT(index >= 0);
  ZX_DEBUG_ASSERT(index < edid::kMaxEdidBlockCount);

  // Size of an E-DDC segment.
  //
  // VESA Enhanced Display Data Channel (E-DDC) Standard version 1.3 revised
  // Dec 31 2020, Section 2.2.5 "Segment Pointer", page 18.
  static constexpr int kEddcSegmentSize = 256;
  static_assert(kEddcSegmentSize == edid::kBlockSize * 2);

  fbl::AutoLock lock(&lock_);

  // `index / 2` is in [0, 127], so casting it to uint8_t won't overflow.
  const uint8_t segment_pointer = static_cast<uint8_t>(index / 2);
  zx::result<> write_segment_result =
      zx::make_result(DpAuxWrite(DP_REQUEST_I2C_WRITE, kDdcSegmentI2cTargetAddress,
                                 /*buf=*/&segment_pointer, /*size=*/1));

  if (write_segment_result.status_value() == ZX_ERR_IO_REFUSED && segment_pointer == 0) {
    // A display device that doesn't support E-DDC returns an I2C NACK response
    // when the host writes to the segment pointer. Thus, we ignore the NACK
    // and perform non-segmented DDC read operations if the segment pointer is
    // zero.
    fdf::info("E-DDC segment pointer is not supported. Will perform DDC read.");
  } else if (write_segment_result.is_error()) {
    fdf::error("Failed to write E-DDC segment pointer: {}", write_segment_result);
    return write_segment_result.take_error();
  }

  // Segment offset of the first byte in the current block.
  //
  // `segment_offset` is either 0 or 128, so casting it to uint8_t won't
  // overflow.
  const uint8_t segment_offset = static_cast<uint8_t>((index % 2) * edid::kBlockSize);
  zx::result<> write_segment_offset_result =
      zx::make_result(DpAuxWrite(DP_REQUEST_I2C_WRITE, kDdcDataI2cTargetAddress,
                                 /*buf=*/&segment_offset, /*size=*/1));
  if (write_segment_offset_result.is_error()) {
    fdf::error("Failed to write E-DDC segment offset: {}", write_segment_offset_result);
    return write_segment_offset_result.take_error();
  }

  zx::result<> read_edid_result = zx::make_result(DpAuxRead(
      DP_REQUEST_I2C_READ, kDdcDataI2cTargetAddress, edid_block.data(), edid_block.size()));
  if (read_edid_result.is_error()) {
    fdf::error("Failed to read E-DDC block #{}: {}", index, read_edid_result);
    return read_edid_result.take_error();
  }

  return zx::ok();
}

bool DpAuxChannelImpl::DpcdRead(uint32_t addr, uint8_t* buf, size_t size) {
  fbl::AutoLock lock(&lock_);
  constexpr uint32_t kReadAttempts = 3;
  for (unsigned i = 0; i < kReadAttempts; i++) {
    if (DpAuxRead(DP_REQUEST_NATIVE_READ, addr, buf, size) == ZX_OK) {
      return true;
    }
    zx_nanosleep(zx_deadline_after(ZX_MSEC(5)));
  }
  return false;
}

bool DpAuxChannelImpl::DpcdWrite(uint32_t addr, const uint8_t* buf, size_t size) {
  fbl::AutoLock lock(&lock_);
  return DpAuxWrite(DP_REQUEST_NATIVE_WRITE, addr, buf, size) == ZX_OK;
}

DpAuxChannelImpl::DpAuxChannelImpl(fdf::MmioBuffer* mmio_buffer, DdiId ddi_id, uint16_t device_id)
    : aux_channel_(mmio_buffer, ddi_id, device_id) {
  ZX_ASSERT(mtx_init(&lock_, mtx_plain) == thrd_success);
}

}  // namespace intel_display
