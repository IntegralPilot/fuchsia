// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Notes on buffering modes
// ------------------------
//
// Threads and strings are cached to improve performance and reduce buffer
// usage. The caching involves emitting separate records that identify
// threads/strings and then referring to them by a numeric id. For performance
// each thread in the application maintains its own cache.
//
// Oneshot: The trace buffer is just one large buffer, and records are written
// until the buffer is full after which all further records are dropped.
//
// Circular:
// The trace buffer is effectively divided into two pieces, and tracing begins
// by writing to the first piece. Once one buffer fills we start writing to the
// other one. This results in half the buffer being dropped at every switch,
// but simplifies things because we don't have to worry about varying record
// lengths.
//
// Streaming:
// The trace buffer is effectively divided into two pieces, and tracing begins
// by writing to the first piece. Once one buffer fills we start writing to the
// other buffer, if it is available, and notify the handler that the buffer is
// full. If the other buffer is not available, then records are dropped until
// it becomes available. The other buffer is unavailable between the point when
// it filled and when the handler reports back that the buffer's contents have
// been saved.
//
// There are two important properties we wish to preserve in circular and
// streaming modes:
// 1) We don't want records describing threads and strings to be dropped:
// otherwise records referring to them will have nothing to refer to.
// 2) We don't want thread records to be dropped at all: Fidelity of recording
// of all traced threads is important, even if some of their records are
// dropped.
// To implement both (1) and (2) we introduce a third buffer that holds
// records we don't want to drop called the "durable buffer". Threads and
// small strings are recorded there. The two buffers holding normal trace
// output are called "rolling buffers", as they fill we roll from one to the
// next. Thread and string records typically aren't very large, the durable
// buffer can hold a lot of records. To keep things simple, until there's a
// compelling reason to do something more, once the durable buffer fills
// tracing effectively stops, and all further records are dropped.
// Note: The term "rolling buffer" is intended to be internal to the trace
// engine/reader/manager and is not intended to appear in public APIs
// (at least not today). Externally, the two rolling buffers comprise the
// "nondurable" buffer.
//
// The protocol between the trace engine and the handler for saving buffers in
// streaming mode is as follows:
// 1) Buffer fills -> handler gets notified via
//    |trace_handler_ops::notify_buffer_full()|. Two arguments are passed
//    along with this request:
//    |wrapped_count| - records how many times tracing has wrapped from one
//    buffer to the next, and also records the current buffer which is the one
//    needing saving. Since there are two rolling buffers, the buffer to save
//    is |wrapped_count & 1|.
//    |durable_data_end| - records how much data has been written to the
//    durable buffer thus far. This data needs to be written before data from
//    the rolling buffers is written so string and thread references work.
// 2) The handler receives the "notify_buffer_full" request.
// 3) The handler saves new durable data since the last time, saves the
//    rolling buffer, and replies back to the engine via
//    |trace_engine_mark_buffer_saved()|.
// 4) The engine receives this notification and marks the buffer as now empty.
//    The next time the engine tries to allocate space from this buffer it will
//    succeed.
// Note that the handler is free to save buffers at whatever rate it can
// manage. The protocol allows for records to be dropped if buffers can't be
// saved fast enough.

#include <assert.h>
#include <inttypes.h>
#include <lib/trace-engine/fields.h>
#include <lib/trace-engine/handler.h>

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <mutex>

#include "buffer.h"
#include "context_impl.h"
#include "zircon/system/ulib/trace-engine/rolling_buffer.h"

namespace trace {
namespace {

// The next context generation number.
std::atomic<uint32_t> g_next_generation{1u};

}  // namespace
}  // namespace trace

trace_context::trace_context(void* buffer, size_t buffer_num_bytes,
                             trace_buffering_mode_t buffering_mode, trace_handler_t* handler)
    : generation_(trace::g_next_generation.fetch_add(1u, std::memory_order_relaxed) + 1u),
      buffering_mode_(buffering_mode),
      buffer_(reinterpret_cast<uint8_t*>(buffer), buffer_num_bytes),
      header_(reinterpret_cast<trace_buffer_header*>(buffer)),
      handler_(handler) {
  ZX_DEBUG_ASSERT(buffer_num_bytes >= kMinPhysicalBufferSize);
  ZX_DEBUG_ASSERT(buffer_num_bytes <= kMaxPhysicalBufferSize);
  ZX_DEBUG_ASSERT(generation_ != 0u);
  ComputeBufferSizes();
  ResetBufferPointers();
}

trace_context::~trace_context() = default;

uint64_t* trace_context::AllocRecord(size_t num_bytes) {
  const AllocationResult alloc = rolling_buffer_.AllocRecord(num_bytes);
  const AllocationVisitor visitor{
      [](Allocation alloc) { return alloc.ptr; },
      [this](BufferFull) -> uint64_t* {
        MarkRecordDropped();
        if (buffering_mode_ == TRACE_BUFFERING_MODE_ONESHOT) {
          // In oneshot mode, if we fill the buffer, there's definitely no more buffer space.
          // If we were the one to fill the buffer, we need to finalize the header.
          std::lock_guard<std::mutex> lock(header_mutex_);
          header_->rolling_data_end[0] = rolling_buffer_.State().GetBufferOffset();
        }
        return nullptr;
      },
      [this](SwitchingAllocation alloc) -> uint64_t* {
        // If the durable buffer is full, it will
        if (unlikely(tracing_artificially_stopped_.load(std::memory_order_relaxed))) {
          return nullptr;
        }
        // If we filled the buffer, we are responsible for switching to the next buffer.
        SetPendingBufferService(alloc.prev_state);
        return alloc.ptr;
      },
  };
  return std::visit(visitor, alloc);
}

// Returns false if there's some reason to not record this record.
void trace_context::SetPendingBufferService(RollingBufferState prev_state) {
  const BufferNumber next_buffer = prev_state.WithNextWrappedCount().GetBufferNumber();
  {
    std::lock_guard<std::mutex> lock(header_mutex_);
    header_->rolling_data_end[static_cast<uint8_t>(prev_state.GetBufferNumber())] =
        prev_state.GetBufferOffset();
    header_->rolling_data_end[static_cast<uint8_t>(next_buffer)] = 0;
    header_->num_records_dropped = num_records_dropped();
    pending_service_.store(prev_state, std::memory_order_relaxed);
  }
}

uint64_t* trace_context::AllocDurableRecord(size_t num_bytes) {
  ZX_DEBUG_ASSERT(UsingDurableBuffer());
  uint64_t* ptr = durable_buffer_.AllocRecord(num_bytes);
  if (likely(ptr != nullptr)) {
    return ptr;
  }
  // A record may be written that relies on this durable record. To preserve data integrity, we
  // disable all further tracing. There is a small window where a non-durable record could get
  // emitted that depends on this durable record. It's rare enough and inconsequential enough that
  // we ignore it.
  MarkTracingArtificiallyStopped();
  return nullptr;
}

bool trace_context::AllocThreadIndex(trace_thread_index_t* out_index) {
  trace_thread_index_t index = next_thread_index_.fetch_add(1u, std::memory_order_relaxed);
  if (unlikely(index > TRACE_ENCODED_THREAD_REF_MAX_INDEX)) {
    // Guard again possible wrapping.
    next_thread_index_.store(TRACE_ENCODED_THREAD_REF_MAX_INDEX + 1u, std::memory_order_relaxed);
    return false;
  }
  *out_index = index;
  return true;
}

bool trace_context::AllocStringIndex(trace_string_index_t* out_index) {
  trace_string_index_t index = next_string_index_.fetch_add(1u, std::memory_order_relaxed);
  if (unlikely(index > TRACE_ENCODED_STRING_REF_MAX_INDEX)) {
    // Guard again possible wrapping.
    next_string_index_.store(TRACE_ENCODED_STRING_REF_MAX_INDEX + 1u, std::memory_order_relaxed);
    return false;
  }
  *out_index = index;
  return true;
}

void trace_context::ComputeBufferSizes() {
  size_t full_buffer_size = buffer_.size();
  ZX_DEBUG_ASSERT(full_buffer_size >= kMinPhysicalBufferSize);
  ZX_DEBUG_ASSERT(full_buffer_size <= kMaxPhysicalBufferSize);
  size_t header_size = sizeof(trace_buffer_header);
  switch (buffering_mode_) {
    case TRACE_BUFFERING_MODE_ONESHOT:
      // Create one big buffer, where durable and non-durable records share
      // the same buffer. There is no separate buffer for durable records.
      durable_buffer_.Set(std::span<uint8_t>{});
      rolling_buffer_.SetOneshotBuffer(
          std::span<uint8_t>{buffer_.data() + header_size, full_buffer_size - header_size});
      break;
    case TRACE_BUFFERING_MODE_CIRCULAR:
    case TRACE_BUFFERING_MODE_STREAMING: {
      // Rather than make things more complex on the user,
      // we choose the sizes of the durable and rolling buffers.
      //
      // Note: The durable buffer must have enough space for at least
      // the initialization record.
      uint64_t avail = full_buffer_size - header_size;
      uint64_t durable_buffer_size = GET_DURABLE_BUFFER_SIZE(avail);
      durable_buffer_size = std::min(durable_buffer_size, DurableBuffer::kMaxDurableBufferSize);
      // Further adjust |durable_buffer_size| to ensure all buffers are a
      // multiple of 8. |full_buffer_size| is guaranteed by
      // |trace_start_engine()| to be a multiple of 4096. We only assume
      // header_size is a multiple of 8. In order for rolling_buffer_size
      // to be a multiple of 8 we need (avail - durable_buffer_size) to be a
      // multiple of 16. Round durable_buffer_size up as necessary.
      uint64_t off_by = (avail - durable_buffer_size) & 15;
      ZX_DEBUG_ASSERT(off_by == 0 || off_by == 8);
      durable_buffer_size += off_by;
      ZX_DEBUG_ASSERT((durable_buffer_size & 7) == 0);
      // The value of |kMinPhysicalBufferSize| ensures this:
      ZX_DEBUG_ASSERT(durable_buffer_size >= DurableBuffer::kMinDurableBufferSize);
      uint64_t rolling_buffer_size = (avail - durable_buffer_size) / 2;
      ZX_DEBUG_ASSERT((rolling_buffer_size & 7) == 0);
      // We need to maintain the invariant that the entire buffer is used.
      // This works if the buffer size is a multiple of
      // sizeof(trace_buffer_header), which is true since the buffer is a
      // vmo (some number of 4K pages).
      ZX_DEBUG_ASSERT(durable_buffer_size + 2 * rolling_buffer_size == avail);
      std::span durable_data = buffer_.subspan(header_size, durable_buffer_size);
      durable_buffer_.Set(durable_data);
      std::span rolling1 =
          std::span{durable_data.data() + durable_buffer_.MaxSize(), rolling_buffer_size};
      std::span rolling2 = std::span{rolling1.data() + rolling_buffer_size, rolling_buffer_size};
      rolling_buffer_.SetDoubleBuffers(rolling1, rolling2);
      break;
    }
    default:
      __UNREACHABLE;
  }
}

void trace_context::ResetBufferPointers() {
  durable_buffer_.Reset();
  rolling_buffer_.Reset();
}

void trace_context::InitBufferHeader() {
  std::lock_guard<std::mutex> lock(header_mutex_);
  memset(header_, 0, sizeof(*header_));

  header_->magic = TRACE_BUFFER_HEADER_MAGIC;
  header_->version = TRACE_BUFFER_HEADER_V0;
  header_->buffering_mode = static_cast<uint8_t>(buffering_mode_);
  header_->total_size = buffer_.size();
  header_->durable_buffer_size = durable_buffer_.MaxSize();
  // The buffers are equal sized.
  header_->rolling_buffer_size = rolling_buffer_.BufferSize(BufferNumber::kZero);
}

void trace_context::ClearEntireBuffer() {
  ResetBufferPointers();
  InitBufferHeader();
}

void trace_context::ClearRollingBuffers() { rolling_buffer_.Reset(); }

void trace_context::UpdateBufferHeaderAfterStopped() {
  std::lock_guard<std::mutex> lock(header_mutex_);
  header_->durable_data_end = durable_buffer_.BytesAllocated();

  RollingBufferState state = rolling_buffer_.State();
  header_->rolling_data_end[static_cast<uint8_t>(state.GetBufferNumber())] =
      state.GetBufferOffset();
  header_->num_records_dropped = num_records_dropped();
  header_->wrapped_count = state.GetWrappedCount();
}

void trace_context::ServiceBuffers() {
  RollingBufferState expected_service = pending_service_.load(std::memory_order_relaxed);
  if (expected_service == RollingBufferState()) {
    // No service required
    return;
  }
  //  There's a pending service, attempt to claim it.
  if (pending_service_.compare_exchange_weak(expected_service, RollingBufferState(),
                                             std::memory_order_relaxed,
                                             std::memory_order_relaxed)) {
    if (buffering_mode_ == TRACE_BUFFERING_MODE_STREAMING) {
      {
        std::lock_guard<std::mutex> lock(header_mutex_);
        RollingBufferState state = rolling_buffer_.State();
        header_->rolling_data_end[static_cast<uint8_t>(state.GetBufferNumber())] =
            state.GetBufferOffset();
        header_->num_records_dropped = num_records_dropped();
        header_->wrapped_count = state.GetWrappedCount();
      }
      // Notify the handler so it starts saving the buffer if we're in streaming mode.
      //
      // Once we receive the notification that the buffer has been saved, we can finish servicing
      // it.
      trace_engine_request_save_buffer(expected_service.GetWrappedCount(),
                                       durable_buffer_.BytesAllocated());
    } else if (buffering_mode_ == TRACE_BUFFERING_MODE_CIRCULAR) {
      // In circular mode, we can immediately mark the buffer has serviced, freeing it for reuse.
      rolling_buffer_.SetBufferServiced(expected_service.GetWrappedCount());
    }
  }
}

void trace_context::MarkTracingArtificiallyStopped() {
  bool expected = false;
  const bool desired = true;
  bool success = tracing_artificially_stopped_.compare_exchange_strong(
      expected, desired, std::memory_order::relaxed, std::memory_order::relaxed);

  // We only want to stop the trace once.
  if (success) {
    {
      std::lock_guard<std::mutex> lock(header_mutex_);
      header_->durable_data_end = durable_buffer_.BytesAllocated();
    }

    // Disable tracing by making it look like the current rolling
    // buffer is full. AllocRecord, on seeing the buffer is full, will
    // then check |tracing_artificially_stopped_|.
    rolling_buffer_.SetBufferFull();
  }
}

void trace_context::HandleSaveRollingBufferRequest(uint32_t wrapped_count,
                                                   uint64_t durable_data_end) {
  // SAFETY: This must only be called after we have observed that there are no additional writers on
  // the buffer we are notifying as full.
  //
  // We achieve this by only calling trace_context::ServiceBuffers after:
  // 1) We have been notified that the buffer is full.
  // 2) We are releasing the trace_context and we observe that we are the only one holding a
  //    context.
  handler_->ops->notify_buffer_full(handler_, wrapped_count, durable_data_end);
}

void trace_context::MarkRollingBufferSaved(uint32_t wrapped_count, uint64_t durable_data_end) {
  std::lock_guard<std::mutex> lock(header_mutex_);
  header_
      ->rolling_data_end[static_cast<uint8_t>(RollingBufferState::ToBufferNumber(wrapped_count))] =
      0;
  rolling_buffer_.SetBufferServiced(wrapped_count);
}

void trace_context::FlushBuffer() {
  if (rolling_buffer_.BytesAllocated() == 0) {
    // Nothing to flush
    return;
  }
  const AllocationResult alloc = rolling_buffer_.Flush();
  const AllocationVisitor visitor{
      [](Allocation alloc) { ZX_DEBUG_ASSERT(false && "Flushing must get switching result"); },
      [](BufferFull) {
        // There's already a pending flush or we're not in streaming mode.
      },
      [this](SwitchingAllocation alloc) { SetPendingBufferService(alloc.prev_state); },
  };
  std::visit(visitor, alloc);
}
