// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_BUFFER_FORWARDER_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_BUFFER_FORWARDER_H_
#include <lib/stdcompat/span.h>
#include <lib/zx/socket.h>

#include <string>
#include <utility>
#include <vector>

#include "src/performance/trace_manager/util.h"

namespace tracing {
class BufferForwarder {
 public:
  static constexpr size_t kBytesWrittenLoggingInterval = 100 * 1024 * 1024;  // 100 MB

  // Default percentage of socket transmit buffer capacity that triggers backpressure.
  // In experimenting with various values, the throughput of the transfer did not change
  // substantially. However, it does provide a measure of safety to avoid critical memory pressure
  // situations. One thing to look out for is the data being transferred can be bursty, so if it
  // does burst over and cause critical memory pressure, consider decreasing the value to be more
  // conservative.
  static constexpr size_t kDefaultSocketBackpressurePercentage = 90;

  explicit BufferForwarder(zx::socket destination,
                           size_t backpressure_percentage = kDefaultSocketBackpressurePercentage)
      : destination_(std::move(destination)), backpressure_percentage_(backpressure_percentage) {}

  // Get or set the socket backpressure percentage threshold.
  size_t backpressure_percentage() const { return backpressure_percentage_; }
  void set_backpressure_percentage(size_t percentage) { backpressure_percentage_ = percentage; }

  // Checks if unread streaming buffers in the socket exceed the backpressure threshold.
  bool IsSocketBackpressureExceeded() const;

  // Pauses until unread socket buffers drain below the backpressure threshold.
  TransferStatus WaitForSocketDrain() const;

  // Write the FxT Magic Bytes to the underlying socket.
  TransferStatus WriteMagicNumberRecord() const;

  TransferStatus WriteProviderInfoRecord(uint32_t provider_id, const std::string& name) const;
  TransferStatus WriteProviderSectionRecord(uint32_t provider_id) const;
  TransferStatus WriteProviderBufferOverflowEvent(uint32_t provider_id) const;

  enum class ForwardStrategy : bool {
    Size,
    Records,
  };

  // Write the records in |buffer| at |vmo_offset| to the output. |size| is the size in bytes of the
  // chunk to examine, which may be more than was written if |strategy| is
  // `ForwardStrategy::Record`. It must always be a multiple of 8.
  //
  // In oneshot mode we assume the end of written records don't look like records and we can just
  // run through the buffer examining records to compute how many are there. This is problematic
  // (without extra effort) in circular and streaming modes as records are written and rewritten.
  // This function handles both cases. If |strategy| is ForwardStrategy::Record then run through the
  // buffer computing the size of each record until we find no more records. If |strategy| is
  // ForwardStrategy::Size then |size| is the number of bytes to write.
  TransferStatus WriteChunkBy(ForwardStrategy strategy, const zx::vmo& vmo, size_t vmo_offset,
                              size_t size) const;

  virtual TransferStatus Flush() { return TransferStatus::kComplete; }
  virtual ~BufferForwarder() = default;

 protected:
  // Writes the contents of |data| to the output socket. Returns
  // TransferStatus::kComplete if the entire buffer has been
  // successfully transferred. A return value of
  // TransferStatus::kReceiverDead indicates that the peer was closed
  // during the transfer.
  virtual TransferStatus WriteBuffer(cpp20::span<const uint8_t> data) const;
  const zx::socket destination_;
  size_t backpressure_percentage_;

 private:
  mutable size_t next_bytes_written_logging_threshold_ = kBytesWrittenLoggingInterval;
  mutable size_t total_bytes_written_ = 0;
};
}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_BUFFER_FORWARDER_H_
