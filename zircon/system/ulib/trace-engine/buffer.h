// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_BUFFER_H_
#define ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_BUFFER_H_

#include <atomic>
#include <span>

// A buffer that, once created and DurableBuffer::Set, can concurrently allocate data to multiple
// writers until it becomes full.
class DurableBuffer {
 public:
  // The minimum size of the durable buffer.
  // There must be enough space for at least the initialization record.
  static constexpr size_t kMinDurableBufferSize = 16;

  // LINT.IfChange
  //
  // The maximum size of the durable buffer.
  // We need enough space for:
  // - initialization record = 16 bytes
  // - string table (max TRACE_ENCODED_STRING_REF_MAX_INDEX = 0x7fffu entries)
  // - thread table (max TRACE_ENCODED_THREAD_REF_MAX_INDEX = 0xff entries)
  // String entries are 8 bytes + length-round-to-8-bytes.
  // Strings have a max size of TRACE_ENCODED_STRING_REF_MAX_LENGTH bytes
  // = 32000. We assume most are < 64 bytes.
  // Thread entries are 8 bytes + pid + tid = 24 bytes.
  // If we assume 10000 registered strings, typically 64 bytes, plus max
  // number registered threads, that works out to:
  // 16 /*initialization record*/
  // + 10000 * (8 + 64) /*strings*/
  // + 255 * 24 /*threads*/
  // = 726136.
  // We round this up to 1MB.
  static constexpr size_t kMaxDurableBufferSize = size_t{1024} * 1024;
  //
  // trace_manager uses this constant to properly size its buffers
  // LINT.ThenChange(//src/performance/trace_manager/tracee.cc)

  // Configure the DurableBuffer to write to the given span.
  void Set(std::span<uint8_t> data);

  // Allocate data from the configured buffer. Thread safe.
  uint64_t* AllocRecord(size_t num_bytes);

  // Return the total number of allocated bytes.
  size_t BytesAllocated() const;

  // Return the total underlying buffer size (allocated and unallocated).
  size_t MaxSize() const;

  // Set the allocation pointer back to the beginning of the buffer.
  void Reset();

 private:
  // A view of the bytes we write into.
  std::span<uint8_t> data_;

  // Current allocation pointer for durable records.
  std::atomic<size_t> bytes_allocated_;
};

#endif  // ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_BUFFER_H_
