// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_CONTEXT_IMPL_H_
#define ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_CONTEXT_IMPL_H_

#include <lib/trace-engine/buffer_internal.h>
#include <lib/trace-engine/context.h>
#include <lib/trace-engine/handler.h>
#include <lib/zx/event.h>
#include <zircon/assert.h>

#include <atomic>
#include <mutex>
#include <span>

#include "buffer.h"
#include "rolling_buffer.h"

// Two preprocessor symbols control what symbols we export in a .so:
// EXPORT and EXPORT_NO_DDK:
// - EXPORT is for symbols exported to both driver and non-driver versions of
//   the library ("non-driver" is the normal case).
// - EXPORT_NO_DDK is for symbols *not* exported in the DDK.
// A third variant is supported which is to export nothing. This is for cases
// like libvulkan which want tracing but do not have access to
// libtrace-engine.so.
// Two preprocessor symbols are provided by the build system to select which
// variant we are building: STATIC_LIBRARY. If it is not defined
// (normal case), or exactly one of them is defined.
#if defined(STATIC_LIBRARY)
#define EXPORT
#define EXPORT_NO_DDK
#else
#define EXPORT __EXPORT
#define EXPORT_NO_DDK __EXPORT
#endif

using trace::internal::trace_buffer_header;

// Return true if there are no buffer acquisitions of the trace context.
bool trace_engine_is_buffer_context_released();

// Called from trace_context to notify the engine a buffer needs saving.
void trace_engine_request_save_buffer(uint32_t wrapped_count, uint64_t durable_data_end);

// Maintains state for a single trace session.
// This structure is accessed concurrently from many threads which hold trace
// context references.
// Implements the opaque type declared in <trace-engine/context.h>.
struct trace_context {
  trace_context(void* buffer, size_t buffer_num_bytes, trace_buffering_mode_t buffering_mode,
                trace_handler_t* handler);

  ~trace_context();

  // Copy the current header state into dest
  void get_header(trace_buffer_header* dest) const {
    std::lock_guard<std::mutex> lock(header_mutex_);
    memcpy(dest, header_, sizeof(trace_buffer_header));
  }

  static size_t min_buffer_size() { return kMinPhysicalBufferSize; }

  static size_t max_buffer_size() { return kMaxPhysicalBufferSize; }

  uint32_t generation() const { return generation_; }

  trace_handler_t* handler() const { return handler_; }

  trace_buffering_mode_t buffering_mode() const { return buffering_mode_; }

  uint64_t num_records_dropped() const {
    return num_records_dropped_.load(std::memory_order_relaxed);
  }

  bool UsingDurableBuffer() const { return buffering_mode_ != TRACE_BUFFERING_MODE_ONESHOT; }

  // Return true if at least one record was dropped.
  bool WasRecordDropped() const { return num_records_dropped() != 0u; }

  void MarkRecordDropped() { num_records_dropped_.fetch_add(1, std::memory_order_relaxed); }

  void ResetRollingBufferPointers();
  void ResetBufferPointers();
  void InitBufferHeader();
  void ClearEntireBuffer();
  void ClearRollingBuffers();
  void UpdateBufferHeaderAfterStopped();

  void ServiceBuffers();

  uint64_t* AllocRecord(size_t num_bytes);
  uint64_t* AllocDurableRecord(size_t num_bytes);
  bool AllocThreadIndex(trace_thread_index_t* out_index);
  bool AllocStringIndex(trace_string_index_t* out_index);

  // This is called by the handler when it has been notified that a buffer
  // has been saved.
  // |wrapped_count| is the wrapped count at the time the buffer save request
  // was made. Similarly for |durable_data_end|.
  void MarkRollingBufferSaved(uint32_t wrapped_count, uint64_t durable_data_end);

  // Requests to prepare the buffer to be read. Only has effect in streaming mode.
  //
  // Normally streaming mode only sends data once it has enough data to require a buffer swap and
  // data flush. This produces fewer, but larger and bursty data transfers. Some clients may want
  // more regular smaller data transfers. These clients may instead manually invoke FlushBuffer
  // whenever they are ready for data.
  //
  // Once received, trace-engine will prepare the buffer to be read, and send a callback
  // requesting a buffer save when it is ready.
  void FlushBuffer();

  // This is only called from the engine to initiate a buffer save.
  void HandleSaveRollingBufferRequest(uint32_t wrapped_count, uint64_t durable_data_end);

 private:
  // The physical buffer must be at least this big.
  // Mostly this is here to simplify buffer size calculations.
  // It's as small as it is to simplify some testcases.
  static constexpr size_t kMinPhysicalBufferSize = 4096;

  // The physical buffer can be at most this big.
  // To keep things simple we ignore the header.
  static constexpr size_t kMaxPhysicalBufferSize = RollingBuffer::kMaxRollingBufferSize;

  // Given a buffer of size |SIZE| in bytes, not including the header,
  // return how much to use for the durable buffer. This is further adjusted
  // to be at most |kMaxDurableBufferSize|, and to account for rolling
  // buffer size alignment constraints.
#define GET_DURABLE_BUFFER_SIZE(size) ((size) / 16)

  // Ensure the smallest buffer is still large enough to hold
  // |kMinDurableBufferSize|.
  static_assert(GET_DURABLE_BUFFER_SIZE(kMinPhysicalBufferSize - sizeof(trace_buffer_header)) >=
                DurableBuffer::kMinDurableBufferSize);

  void ComputeBufferSizes();

  void SetPendingBufferService(RollingBufferState prev_state);

  void MarkTracingArtificiallyStopped();

  // The generation counter associated with this context to distinguish
  // it from previously created contexts.
  uint32_t const generation_;

  // The buffering mode.
  trace_buffering_mode_t const buffering_mode_;

  // The entire physical buffer.
  const std::span<uint8_t> buffer_;

  // Aliases with buffer_, but as a trace_buffer_header*
  //
  // We don't write the header as atomics and we can write to the header in both trace writing
  // threads in the case of a buffer switch as well as on the trace-engine async loop.
  // Coordinating writing them locklessly would be difficult, we grab a mutex.
  mutable std::mutex header_mutex_;
  trace_buffer_header* const header_ __TA_GUARDED(header_mutex_);

  // Durable-record buffer
  //
  // This only used in circular and streaming modes: There is no separate
  // buffer for durable records in oneshot mode.
  DurableBuffer durable_buffer_;

  // General record buffer.
  RollingBuffer rolling_buffer_;

  std::atomic<RollingBufferState> pending_service_{RollingBufferState()};

  // A count of the number of records that have been dropped.
  std::atomic<uint64_t> num_records_dropped_{0};

  // Set to true if the engine needs to stop tracing for some reason.
  std::atomic<bool> tracing_artificially_stopped_{false};

  // Handler associated with the trace session.
  trace_handler_t* const handler_;

  // The next thread index to be assigned.
  std::atomic<trace_thread_index_t> next_thread_index_{TRACE_ENCODED_THREAD_REF_MIN_INDEX};

  // The next string table index to be assigned.
  std::atomic<trace_string_index_t> next_string_index_{TRACE_ENCODED_STRING_REF_MIN_INDEX};
};

#endif  // ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_CONTEXT_IMPL_H_
