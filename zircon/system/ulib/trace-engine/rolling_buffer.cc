// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rolling_buffer.h"

#include <zircon/assert.h>

#include <algorithm>
#include <optional>

#include "lib/trace-engine/fields.h"
#include "lib/trace-engine/types.h"

/// A lockless multi-writer single reader for trace-engine
///
/// The algorithm presented here is an abstraction over double buffering. Once one buffer fills, we
/// switch to a new buffer, and the previous buffer is not writable until marked serviced.
///
/// The caller is responsible for managing the servicing of buffers and notifying us once the buffer
/// is serviced and can be reused.
///
/// The implementation of the buffer coordinates on a single uint64_t atomic which only tracks the
/// allocated memory, it doesn't have a way to commit writes. This has a few consequences:
/// - A writer which receives a successful allocation MUST write data its reservation.
/// - We need to use a cmpxchg loop to increment the allocation pointer so that we only increment
///   the pointer if we actually have space to do so.
/// - We don't know when data we've allocated becomes valid. The caller is responsible for tracking
///   this.
/// - We can get away with fully relaxed reads and writes. We only ever read/write a single atomic
///   to manage allocation so ordering between other loads and store doesn't matter.
namespace {
BufferNumber NextBuffer(BufferNumber b) {
  switch (b) {
    case BufferNumber::kZero:
      return BufferNumber::kOne;
    case BufferNumber::kOne:
      return BufferNumber::kZero;
  }
}
}  // namespace

void RollingBuffer::Reset() { state_.store(RollingBufferState{}, std::memory_order_relaxed); }

size_t RollingBuffer::BytesAllocated() const {
  RollingBufferState state = state_.load(std::memory_order_relaxed);
  return state.GetBufferOffset();
}

void RollingBuffer::SetBufferFull() {
  RollingBufferState expected = state_.load(std::memory_order_relaxed);
  RollingBufferState desired;
  do {
    const BufferNumber current_buffer = expected.GetBufferNumber();
    const BufferNumber next_buffer_number = NextBuffer(current_buffer);
    desired = expected.WithBufferMarkedFilled(next_buffer_number)
                  .WithOffset(static_cast<uint32_t>(
                      rolling_buffers_[static_cast<uint8_t>(current_buffer)].size_bytes()));
  } while (!state_.compare_exchange_weak(expected, desired, std::memory_order_relaxed,
                                         std::memory_order_relaxed));

  // If tracing is stopped artificially, the rolling buffer offset is advanced to the end of the
  // buffer so that future writes fail and the buffer is reported as full. However, doing so leaves
  // unallocated space between the previous offset and the end of the buffer unwritten. Trace
  // readers reading up to rolling_data_end will encounter this unwritten memory and fail to parse.
  // Fill the remainder of the buffer with placeholder BLOB records so that parsers can cleanly
  // consume the entire buffer without error (https://fxbug.dev/553091090).
  const BufferNumber current_buffer = expected.GetBufferNumber();
  const uint32_t prev_offset = expected.GetBufferOffset();
  const size_t buffer_size = rolling_buffers_[static_cast<uint8_t>(current_buffer)].size_bytes();

  if (prev_offset < buffer_size) {
    size_t remaining_bytes = buffer_size - prev_offset;
    ZX_DEBUG_ASSERT((remaining_bytes & 7) == 0);
    size_t remaining_words = remaining_bytes / sizeof(uint64_t);
    uint64_t* write_ptr = reinterpret_cast<uint64_t*>(
        rolling_buffers_[static_cast<uint8_t>(current_buffer)].data() + prev_offset);

    while (remaining_words > 0) {
      const size_t chunk_words =
          std::min(remaining_words, static_cast<size_t>(trace::RecordFields::kMaxRecordSizeWords));
      const size_t payload_bytes = (chunk_words - 1) * sizeof(uint64_t);
      const uint64_t header =
          trace::RecordFields::Type::Make(trace::ToUnderlyingType(trace::RecordType::kBlob)) |
          trace::RecordFields::RecordSize::Make(chunk_words) |
          trace::BlobRecordFields::NameStringRef::Make(TRACE_ENCODED_STRING_REF_EMPTY) |
          trace::BlobRecordFields::BlobSize::Make(payload_bytes) |
          trace::BlobRecordFields::BlobType::Make(TRACE_BLOB_TYPE_DATA);

      *write_ptr = header;
      write_ptr += chunk_words;
      remaining_words -= chunk_words;
    }
  }
}

bool RollingBuffer::SetBufferServiced(uint32_t wrapped_count) {
  RollingBufferState expected = state_.load(std::memory_order_relaxed);
  RollingBufferState desired;
  do {
    const uint32_t current_wrapped = expected.GetWrappedCount();
    const BufferNumber buffer_serviced = RollingBufferState::ToBufferNumber(wrapped_count);
    if (current_wrapped != wrapped_count + 1 || !expected.IsBufferFilled(buffer_serviced)) {
      // Someone beat us to marking the buffer serviced
      return false;
    }
    desired = expected.WithBufferMarkedServiced(buffer_serviced);
  } while (!state_.compare_exchange_weak(expected, desired, std::memory_order_relaxed,
                                         std::memory_order_relaxed));
  return true;
}

AllocationResult RollingBuffer::AllocRecord(size_t num_bytes) {
  ZX_DEBUG_ASSERT((num_bytes & 7) == 0);
  static_assert(TRACE_ENCODED_INLINE_LARGE_RECORD_MAX_SIZE < kMaxRollingBufferSize);

  RollingBufferState expected = state_.load(std::memory_order_relaxed);
  if (unlikely(num_bytes > TRACE_ENCODED_INLINE_LARGE_RECORD_MAX_SIZE)) {
    return BufferFull{};
  }

  // We need to use a cmpxchg loop here instead of a fetch_add. Since we don't have a
  // commit pointer and rely on the writer to actually write the bytes, we don't want to modify the
  // bytes allocated unless we know the writer is also going to be able to write to the allocated
  // space.
  while (true) {
    const uint32_t buffer_offset = expected.GetBufferOffset();
    const BufferNumber buffer_number = expected.GetBufferNumber();
    ZX_DEBUG_ASSERT(!expected.IsBufferFilled(buffer_number));

    std::optional<RollingBufferState> service_required = std::nullopt;
    RollingBufferState desired;

    if (unlikely(buffer_offset + num_bytes >
                 rolling_buffers_[static_cast<uint8_t>(buffer_number)].size_bytes())) {
      // We don't fit in the current buffer, we'll need to roll to the next buffer.
      //
      // Start with some checks to ensure that we can actually roll.
      if (expected.IsBufferFilled(NextBuffer(buffer_number))) {
        // The next buffer is full too, there's no room, just return.
        //
        // NOTE: Since this doesn't modify the state, writers will continue to attempt writing, each
        // time checking if the other buffer has been serviced yet. This results in each write
        // polling if the buffer is ready or not, and once the buffer is serviced, we'll allocate
        // and roll the buffer.
        return BufferFull{};
      }
      // When we're SetOneshot'd, the second buffer is zero sized. The caller will never service the
      // buffers, so just return.
      if (rolling_buffers_[1].size_bytes() == 0) {
        return BufferFull{};
      }
      if (num_bytes >
          rolling_buffers_[static_cast<uint8_t>(NextBuffer(buffer_number))].size_bytes()) {
        return BufferFull{};
      }
      service_required = std::optional<RollingBufferState>{expected};
      // To roll the buffer, we need to:
      //
      // 1) Mark the current buffer as filled/requiring service.
      // 2) Increment the wrapped count.
      // 3) Allocate space in the next buffer.
      desired = expected.WithBufferMarkedFilled(buffer_number)
                    .WithOffset(static_cast<uint32_t>(num_bytes))
                    .WithNextWrappedCount();
    } else {
      if (unlikely(num_bytes >
                   rolling_buffers_[static_cast<uint8_t>(buffer_number)].size_bytes())) {
        return BufferFull{};
      }
      desired = expected.WithAllocatedBytes(static_cast<uint32_t>(num_bytes));
    }

    const bool success = state_.compare_exchange_weak(expected, desired, std::memory_order_relaxed,
                                                      std::memory_order_relaxed);
    if (success) {
      const BufferNumber buffer_number = desired.GetBufferNumber();
      const uint32_t offset = desired.GetBufferOffset();
      uint8_t* const ptr =
          rolling_buffers_[static_cast<uint8_t>(buffer_number)].data() + offset - num_bytes;
      if (service_required) {
        return SwitchingAllocation{.ptr = reinterpret_cast<uint64_t*>(ptr),
                                   .prev_state = service_required.value()};
      }
      return Allocation{reinterpret_cast<uint64_t*>(ptr)};
    }
  }
}

AllocationResult RollingBuffer::Flush() {
  RollingBufferState expected = state_.load(std::memory_order_relaxed);

  // Flushing is similar to a regular allocation except that always roll to the next buffer and
  // don't write any data to the allocation that we get.
  while (true) {
    const BufferNumber buffer_number = expected.GetBufferNumber();
    ZX_DEBUG_ASSERT(!expected.IsBufferFilled(buffer_number));

    std::optional<RollingBufferState> service_required = std::nullopt;
    RollingBufferState desired;

    if (expected.IsBufferFilled(NextBuffer(buffer_number))) {
      // We're already trying to flush the buffers. Do nothing.
      return BufferFull{};
    }

    // We're not in streaming mode, so we can't flush the buffers
    if (rolling_buffers_[1].size_bytes() == 0) {
      return BufferFull{};
    }

    service_required = std::optional<RollingBufferState>{expected};
    // To roll the buffer, we need to:
    //
    // 1) Mark the current buffer as filled/requiring service.
    // 2) Increment the wrapped count.
    // 3) Allocate space in the next buffer.
    desired = expected.WithBufferMarkedFilled(buffer_number)
                  .WithOffset(static_cast<uint32_t>(0))
                  .WithNextWrappedCount();

    const bool success = state_.compare_exchange_weak(expected, desired, std::memory_order_relaxed,
                                                      std::memory_order_relaxed);
    if (success) {
      const BufferNumber buffer_number = desired.GetBufferNumber();
      const uint32_t offset = desired.GetBufferOffset();
      uint8_t* const ptr = rolling_buffers_[static_cast<uint8_t>(buffer_number)].data() + offset;
      return SwitchingAllocation{.ptr = reinterpret_cast<uint64_t*>(ptr),
                                 .prev_state = service_required.value()};
    }
  }
}
