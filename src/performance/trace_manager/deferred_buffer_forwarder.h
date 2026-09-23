// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_DEFERRED_BUFFER_FORWARDER_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_DEFERRED_BUFFER_FORWARDER_H_
#include <lib/stdcompat/span.h>
#include <lib/zx/socket.h>

#include <filesystem>
#include <vector>

#include "src/performance/trace_manager/buffer_forwarder.h"
#include "src/performance/trace_manager/util.h"

namespace tracing {
// A version of the BufferForwarder which first buffers to the local filesystem before flushing to
// the resulting socked
class DeferredBufferForwarder : public BufferForwarder {
 public:
  // Use a fixed buffer size to move the data from the file to the socket. This avoids a large
  // stack allocation and reduces the number of reads to copy the data.
  static constexpr size_t kDefaultTransferBufferSize = 64 * 1024;

  explicit DeferredBufferForwarder(
      zx::socket destination, size_t transfer_buffer_size = kDefaultTransferBufferSize,
      size_t backpressure_percentage = BufferForwarder::kDefaultSocketBackpressurePercentage);

  // Get or set the transfer buffer size for controlled batching during flush.
  size_t transfer_buffer_size() const { return chunk_buffer_.size(); }
  void set_transfer_buffer_size(size_t size) {
    if (size > 0) {
      chunk_buffer_.resize(size);
    }
  }

  TransferStatus Flush() override;

  ~DeferredBufferForwarder() override;

  // Protected for testing
 protected:
  // Writes the contents of |data| to a temporary location on disk. Data can then be flushed to the
  // socket once the trace is complete.
  //
  // Returns TransferStatus::kComplete if the entire buffer has been
  // successfully transferred. A return value of
  // TransferStatus::kReceiverDead indicates that the peer was closed
  // during the transfer.
  TransferStatus WriteBuffer(cpp20::span<const uint8_t> data) const override;
  bool flushed_{false};

 private:
  std::filesystem::path buffer_path_;
  FILE* buffer_file_;
  std::vector<uint8_t> chunk_buffer_;
};
}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_DEFERRED_BUFFER_FORWARDER_H_
