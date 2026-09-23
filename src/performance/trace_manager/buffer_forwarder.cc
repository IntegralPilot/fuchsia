// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/buffer_forwarder.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/fields.h>
#include <lib/zx/time.h>
#include <lib/zx/vmar.h>

namespace tracing {

bool BufferForwarder::IsSocketBackpressureExceeded() const {
  zx_info_socket_t info;
  if (destination_.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr) == ZX_OK) {
    size_t threshold = std::max<size_t>(1, (info.tx_buf_max * backpressure_percentage_) / 100);
    return info.tx_buf_size >= threshold;
  }
  return false;
}

TransferStatus BufferForwarder::WaitForSocketDrain() const {
  while (IsSocketBackpressureExceeded()) {
    zx_signals_t pending = 0;
    // Set the deadline to 10ms. This is short enough to allow responsiveness
    // but long enough to allow the reader to be scheduled and drain some data.
    zx_status_t status =
        destination_.wait_one(ZX_SOCKET_PEER_CLOSED, zx::deadline_after(zx::msec(10)), &pending);
    if (status == ZX_ERR_TIMED_OUT) {
      continue;
    }
    if (status != ZX_OK || (pending & ZX_SOCKET_PEER_CLOSED)) {
      return TransferStatus::kReceiverDead;
    }
  }
  return TransferStatus::kComplete;
}

TransferStatus BufferForwarder::WriteMagicNumberRecord() const {
  size_t num_words = 1u;
  uint64_t record = trace::MagicNumberRecordFields::Type::Make(
                        trace::ToUnderlyingType(trace::RecordType::kMetadata)) |
                    trace::MagicNumberRecordFields::RecordSize::Make(num_words) |
                    trace::MagicNumberRecordFields::MetadataType::Make(
                        trace::ToUnderlyingType(trace::MetadataType::kTraceInfo)) |
                    trace::MagicNumberRecordFields::TraceInfoType::Make(
                        trace::ToUnderlyingType(trace::TraceInfoType::kMagicNumber)) |
                    trace::MagicNumberRecordFields::Magic::Make(trace::kMagicValue);
  return WriteBuffer({reinterpret_cast<uint8_t*>(&record), trace::WordsToBytes(num_words)});
}

TransferStatus BufferForwarder::WriteProviderInfoRecord(uint32_t provider_id,
                                                        const std::string& name) const {
  size_t num_words = 1u + trace::BytesToWords(trace::Pad(name.size()));
  std::vector<uint64_t> record(num_words);
  record[0] = trace::ProviderInfoMetadataRecordFields::Type::Make(
                  trace::ToUnderlyingType(trace::RecordType::kMetadata)) |
              trace::ProviderInfoMetadataRecordFields::RecordSize::Make(num_words) |
              trace::ProviderInfoMetadataRecordFields::MetadataType::Make(
                  trace::ToUnderlyingType(trace::MetadataType::kProviderInfo)) |
              trace::ProviderInfoMetadataRecordFields::Id::Make(provider_id) |
              trace::ProviderInfoMetadataRecordFields::NameLength::Make(name.size());
  memcpy(&record[1], name.c_str(), name.size());
  return WriteBuffer({reinterpret_cast<uint8_t*>(record.data()), trace::WordsToBytes(num_words)});
}

TransferStatus BufferForwarder::WriteProviderSectionRecord(uint32_t provider_id) const {
  size_t num_words = 1u;
  uint64_t record = trace::ProviderSectionMetadataRecordFields::Type::Make(
                        trace::ToUnderlyingType(trace::RecordType::kMetadata)) |
                    trace::ProviderSectionMetadataRecordFields::RecordSize::Make(num_words) |
                    trace::ProviderSectionMetadataRecordFields::MetadataType::Make(
                        trace::ToUnderlyingType(trace::MetadataType::kProviderSection)) |
                    trace::ProviderSectionMetadataRecordFields::Id::Make(provider_id);
  return WriteBuffer({reinterpret_cast<uint8_t*>(&record), trace::WordsToBytes(num_words)});
}

TransferStatus BufferForwarder::WriteProviderBufferOverflowEvent(uint32_t provider_id) const {
  size_t num_words = 1u;
  uint64_t record = trace::ProviderEventMetadataRecordFields::Type::Make(
                        trace::ToUnderlyingType(trace::RecordType::kMetadata)) |
                    trace::ProviderEventMetadataRecordFields::RecordSize::Make(num_words) |
                    trace::ProviderEventMetadataRecordFields::MetadataType::Make(
                        trace::ToUnderlyingType(trace::MetadataType::kProviderEvent)) |
                    trace::ProviderEventMetadataRecordFields::Id::Make(provider_id) |
                    trace::ProviderEventMetadataRecordFields::Event::Make(
                        trace::ToUnderlyingType(trace::ProviderEventType::kBufferOverflow));
  return WriteBuffer({reinterpret_cast<uint8_t*>(&record), trace::WordsToBytes(num_words)});
}

uint64_t GetBufferWordsWritten(const uint64_t* buffer, uint64_t size_in_words) {
  const uint64_t* start = buffer;
  const uint64_t* current = start;
  const uint64_t* end = start + size_in_words;

  while (current < end) {
    auto type = trace::RecordFields::Type::Get<trace::RecordType>(*current);
    uint64_t length;
    if (type != trace::RecordType::kLargeRecord) {
      length = trace::RecordFields::RecordSize::Get<size_t>(*current);
    } else {
      length = trace::LargeBlobFields::RecordSize::Get<size_t>(*current);
    }

    if (length == 0 || length > trace::RecordFields::kMaxRecordSizeBytes ||
        current + length >= end) {
      break;
    }
    current += length;
  }

  return current - start;
}

TransferStatus BufferForwarder::WriteChunkBy(BufferForwarder::ForwardStrategy strategy,
                                             const zx::vmo& vmo, uint64_t vmo_offset,
                                             uint64_t size) const {
  FX_LOGS(DEBUG) << ": Writing chunk: vmo offset 0x" << std::hex << vmo_offset << ", size 0x"
                 << std::hex << size
                 << (strategy == ForwardStrategy::Size ? ", by-size" : ", by-record");

  // TODO(gmtr): This is run on the async loop and we may block on the socket write below. We should
  // instead write as much as possible to the socket and if we get ZX_SHOULD_WAIT we instead
  // schedule a continuation on the async loop when the socket becomes writable.

  uint64_t size_in_words = trace::BytesToWords(size);
  // For paranoia purposes verify size is a multiple of the word size so we
  // don't risk overflowing the buffer later.
  FX_DCHECK(trace::WordsToBytes(size_in_words) == size);

  // The passed in vmo_offset isn't necessarily page aligned. Unlike zx_vmar_read, zx_vmar_map
  // requires a page aligned offset, so we'll need to fudge the offset a bit. Truncate the offset to
  // be page aligned and then add back the extra bytes to the size of the region that we are mapping
  // and then also offset the addr we get back from the mapping.
  uint64_t page_aligned_offset = vmo_offset & (~(zx_system_get_page_size() - 1));
  uint64_t page_aligned_remainder = vmo_offset % zx_system_get_page_size();

  zx_vaddr_t addr;
  size_t map_size = size + page_aligned_remainder;
  zx_status_t map_result =
      zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, page_aligned_offset, map_size, &addr);
  if (map_result != ZX_OK) {
    FX_PLOGS(ERROR, map_result) << "Failed to read data from buffer_vmo: "
                                << "offset=" << page_aligned_offset << ", size=" << size;
    return TransferStatus::kProviderError;
  }
  auto d = fit::defer([addr, map_size]() { zx::vmar::root_self()->unmap(addr, map_size); });

  zx_vaddr_t offset_addr = addr + page_aligned_remainder;
  uint64_t bytes_written;
  if (strategy == BufferForwarder::ForwardStrategy::Records) {
    uint64_t words_written =
        GetBufferWordsWritten(reinterpret_cast<const uint64_t*>(offset_addr), size_in_words);
    bytes_written = trace::WordsToBytes(words_written);
    FX_LOGS(DEBUG) << "By-record -> " << bytes_written << " bytes";
  } else {
    bytes_written = size;
  }

  return WriteBuffer({reinterpret_cast<const uint8_t*>(offset_addr), bytes_written});
}

TransferStatus BufferForwarder::WriteBuffer(cpp20::span<const uint8_t> data) const {
  while (!data.empty()) {
    if (TransferStatus status = WaitForSocketDrain(); status != TransferStatus::kComplete) {
      return status;
    }

    size_t actual = 0;
    if (zx_status_t status = destination_.write(0u, data.data(), data.size(), &actual);
        status != ZX_OK) {
      if (status == ZX_ERR_SHOULD_WAIT) {
        zx_signals_t pending = 0;
        if (zx_status_t status = destination_.wait_one(ZX_SOCKET_WRITABLE | ZX_SOCKET_PEER_CLOSED,
                                                       zx::time::infinite(), &pending);
            status != ZX_OK) {
          FX_PLOGS(ERROR, status) << "Wait on socket failed";
          return TransferStatus::kWriteError;
        }

        if (pending & ZX_SOCKET_WRITABLE) {
          continue;
        }

        if (pending & ZX_SOCKET_PEER_CLOSED) {
          FX_PLOGS(ERROR, status) << "Peer closed while writing to socket";
          return TransferStatus::kReceiverDead;
        }
      }

      if (status == ZX_ERR_PEER_CLOSED) {
        FX_PLOGS(ERROR, status) << "Peer closed while writing to socket";
        return TransferStatus::kReceiverDead;
      }

      FX_PLOGS(ERROR, status) << "Write returned";
      return TransferStatus::kWriteError;
    }
    data = data.subspan(actual);
    total_bytes_written_ += actual;
    if (total_bytes_written_ >= next_bytes_written_logging_threshold_) {
      size_t bytes_written_interval_crossed =
          (total_bytes_written_ / kBytesWrittenLoggingInterval) * kBytesWrittenLoggingInterval;
      size_t interval_crossed_mb = bytes_written_interval_crossed / (1024 * 1024);
      FX_LOGS(INFO) << "Trace size: streamed " << interval_crossed_mb << " MB";
      next_bytes_written_logging_threshold_ =
          bytes_written_interval_crossed + kBytesWrittenLoggingInterval;
    }
  }

  return TransferStatus::kComplete;
}

}  // namespace tracing
