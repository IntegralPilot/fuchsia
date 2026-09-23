// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <iterator>
#include <memory>
#include <sstream>
#include <utility>

#include <trace-reader/reader_internal.h>

namespace trace {
namespace internal {

std::string BufferHeaderReader::Create(const void* header, size_t buffer_size,
                                       std::unique_ptr<BufferHeaderReader>* out_reader) {
  if (buffer_size < sizeof(trace_buffer_header)) {
    return "buffer too small for header";
  }
  auto hdr = reinterpret_cast<const trace_buffer_header*>(header);
  auto error = Validate(*hdr, buffer_size);
  if (!error.empty()) {
    return error;
  }
  *out_reader = std::unique_ptr<BufferHeaderReader>(new BufferHeaderReader(hdr));
  return "";
}

BufferHeaderReader::BufferHeaderReader(const trace_buffer_header* header) : header_(header) {}

std::string BufferHeaderReader::Validate(const trace_buffer_header& header, size_t buffer_size) {
  if (header.magic != TRACE_BUFFER_HEADER_MAGIC) {
    return (std::stringstream() << "bad magic: 0x" << std::hex << header.magic).str();
  }
  if (header.version != TRACE_BUFFER_HEADER_V0) {
    return (std::stringstream() << "bad version: " << header.version).str();
  }

  if (buffer_size & 7u) {
    std::stringstream ss;
    ss << "buffer size not multiple of 64-bit words: 0x" << std::hex << buffer_size;
    return ss.str();
  }

  switch (header.buffering_mode) {
    case TRACE_BUFFERING_MODE_ONESHOT:
    case TRACE_BUFFERING_MODE_CIRCULAR:
    case TRACE_BUFFERING_MODE_STREAMING:
      break;
    default:
      return (std::stringstream() << "bad buffering mode: " << header.buffering_mode).str();
  }

  if (header.total_size != buffer_size) {
    std::stringstream ss;
    ss << "bad total buffer size: 0x" << std::hex << header.total_size;
    return ss.str();
  }

  auto rolling_buffer_size = header.rolling_buffer_size;
  auto durable_buffer_size = header.durable_buffer_size;

  if ((rolling_buffer_size & 7) != 0) {
    std::stringstream ss;
    ss << "bad rolling buffer size: 0x" << std::hex << rolling_buffer_size;
    return ss.str();
  }
  if ((durable_buffer_size & 7) != 0) {
    std::stringstream ss;
    ss << "bad durable buffer size: 0x" << std::hex << durable_buffer_size;
    return ss.str();
  }

  if (header.buffering_mode == TRACE_BUFFERING_MODE_ONESHOT) {
    if (rolling_buffer_size != buffer_size - sizeof(trace_buffer_header)) {
      std::stringstream ss;
      ss << "bad rolling buffer size: 0x" << std::hex << rolling_buffer_size;
      return ss.str();
    }
    if (durable_buffer_size != 0) {
      std::stringstream ss;
      ss << "bad durable buffer size: 0x" << std::hex << durable_buffer_size;
      return ss.str();
    }
  } else {
    if (rolling_buffer_size >= buffer_size / 2) {
      std::stringstream ss;
      ss << "bad rolling buffer size: 0x" << std::hex << rolling_buffer_size;
      return ss.str();
    }
    if (durable_buffer_size >= rolling_buffer_size) {
      std::stringstream ss;
      ss << "bad durable buffer size: 0x" << std::hex << durable_buffer_size;
      return ss.str();
    }
    if ((sizeof(trace_buffer_header) + durable_buffer_size + (2 * rolling_buffer_size)) !=
        buffer_size) {
      std::stringstream ss;
      ss << "buffer sizes don't add up: 0x" << std::hex << durable_buffer_size << ", 0x" << std::hex
         << rolling_buffer_size;
      return ss.str();
    }
  }

  for (size_t i = 0; i < std::size(header.rolling_data_end); ++i) {
    auto data_end = header.rolling_data_end[i];
    if (data_end > rolling_buffer_size || (data_end & 7) != 0) {
      std::stringstream ss;
      ss << "bad data end for buffer " << i << ": 0x" << std::hex << data_end << " (size 0x"
         << std::hex << rolling_buffer_size << ")";
      return ss.str();
    }
  }

  auto durable_data_end = header.durable_data_end;
  if (durable_data_end > durable_buffer_size || (durable_data_end & 7) != 0) {
    std::stringstream ss;
    ss << "bad durable_data_end: 0x" << std::hex << durable_data_end;
    return ss.str();
  }

  return "";
}

TraceBufferReader::TraceBufferReader(ChunkConsumer chunk_consumer, ErrorHandler error_handler)
    : chunk_consumer_(std::move(chunk_consumer)), error_handler_(std::move(error_handler)) {}

bool TraceBufferReader::ReadChunks(const void* buffer, size_t buffer_size) {
  std::unique_ptr<BufferHeaderReader> header;
  auto error = BufferHeaderReader::Create(buffer, buffer_size, &header);
  if (!error.empty()) {
    error_handler_(error);
    return false;
  }

  CallChunkConsumerIfNonEmpty(BufferHeaderReader::GetDurableBuffer(buffer),
                              header->durable_data_end());

  // There's only two buffers, thus the earlier one is not the current one.
  // It's important to process them in chronological order on the off
  // chance that the earlier buffer provides a stringref or threadref
  // referenced by the later buffer.
  int later_buffer = BufferHeaderReader::GetBufferNumber(header->wrapped_count());
  int earlier_buffer = 0;
  if (header->wrapped_count() > 0)
    earlier_buffer = BufferHeaderReader::GetBufferNumber(header->wrapped_count() - 1);

  if (earlier_buffer != later_buffer) {
    CallChunkConsumerIfNonEmpty(header->GetRollingBuffer(buffer, earlier_buffer),
                                header->rolling_data_end(earlier_buffer));
  }

  CallChunkConsumerIfNonEmpty(header->GetRollingBuffer(buffer, later_buffer),
                              header->rolling_data_end(later_buffer));

  return true;
}

void TraceBufferReader::CallChunkConsumerIfNonEmpty(const void* buf_ptr, size_t size) {
  if (size != 0) {
    auto word_size = sizeof(uint64_t);
    ZX_DEBUG_ASSERT((reinterpret_cast<uintptr_t>(buf_ptr) & (word_size - 1)) == 0);
    ZX_DEBUG_ASSERT((size & (word_size - 1)) == 0);
    Chunk chunk(reinterpret_cast<const uint64_t*>(buf_ptr), size / word_size);
    chunk_consumer_(chunk);
  }
}

}  // namespace internal
}  // namespace trace
