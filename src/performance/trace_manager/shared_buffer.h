// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_SHARED_BUFFER_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_SHARED_BUFFER_H_

#include <fidl/fuchsia.tracing/cpp/common_types.h>
#include <lib/trace-engine/types.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <trace-reader/reader_internal.h>

#include "src/performance/trace_manager/buffer_forwarder.h"
#include "src/performance/trace_manager/util.h"

const char* ModeName(fuchsia_tracing::BufferingMode mode);

struct TransferStats {
  uint8_t durable_used_percent;
  uint32_t wrapped_count;
  uint64_t records_dropped;
  uint64_t non_durable_bytes_written;
};

class SharedBuffer {
 public:
  static zx::result<SharedBuffer> Create(size_t buffer_size,
                                         fuchsia_tracing::BufferingMode buffering_mode,
                                         std::string provider_name, uint32_t provider_id);
  zx::unowned_vmo Vmo() const;
  fuchsia_tracing::BufferingMode BufferingMode() const;

  // Transfer records according to the streaming mode algorithm
  //
  // Transfers not yet transferred durable records up to the new durable_data_end.
  // Transfers not yet transferred records up the buffer given by wrapped count.
  bool StreamingTransfer(const std::shared_ptr<const tracing::BufferForwarder>& output,
                         uint32_t wrapped_count, uint64_t durable_data_end);

  // Transfer all records in the buffer.
  std::pair<tracing::TransferStatus, TransferStats> TransferAll(
      const std::shared_ptr<const tracing::BufferForwarder>& output) const;

  SharedBuffer(SharedBuffer&&) = default;
  SharedBuffer& operator=(SharedBuffer&&) = default;

 private:
  SharedBuffer(zx::vmo vmo, std::string provider_name,
               fuchsia_tracing::BufferingMode buffering_mode, size_t buffer_size,
               uint32_t provider_id);

  bool VerifyBufferHeader(const trace::internal::BufferHeaderReader* header) const;
  tracing::TransferStatus WriteProviderIdRecord(
      const std::shared_ptr<const tracing::BufferForwarder>& output) const;
  tracing::TransferStatus WriteChunk(const std::shared_ptr<const tracing::BufferForwarder>& output,
                                     uint64_t offset, uint64_t last, uint64_t end,
                                     uint64_t buffer_size) const;

  zx::vmo vmo_;
  // Used only for providing debugging information.
  std::string provider_name_;
  fuchsia_tracing::BufferingMode buffering_mode_;
  size_t buffer_size_;

  mutable bool provider_info_record_written_ = false;

  uint32_t last_wrapped_count_ = 0u;
  uint64_t last_durable_data_end_ = 0;
  uint32_t provider_id_;
};
#endif  // SRC_PERFORMANCE_TRACE_MANAGER_SHARED_BUFFER_H_
