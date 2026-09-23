// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_HEAP_PROFILE_INCLUDE_LIB_HEAP_PROFILE_H_
#define ZIRCON_KERNEL_LIB_HEAP_PROFILE_INCLUDE_LIB_HEAP_PROFILE_H_
#include <lib/page/size.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <tuple>

#include <fbl/intrusive_hash_table.h>

// WARNING: This defines a private and unstable binary interface for a user-space consumer.
//
// The Rust-based consumer relies on the exact memory layout, and access patterns defined here.
// Because this layout is duplicated manually (not generated), the compiler provides no safety
// against ABI mismatches. Changes to this contract must be carefully synchronized.

// This is subject to change in every kernel version and is not meant to be any kind of stable ABI
// between the kernel and userland. The expectation is that this layout will be used only by a
// single privileged service that is tightly-coupled with the kernel, i.e. always built from source
// when building the kernel.

// Memory Layout and Visibility Guarantees
// ---------------------------------------

// The heap profile consists of a sequence of variable-size records stored
// consecutively in a fixed-size buffer.
//
// Buffer Layout:
//   - Header (8 bytes)
//   - 0 or more variable-size records
//   - Remaining space is zeroed
//
// Record Layout:
//   - Counters (32 bytes)
//   - Backtrace Size N (8 bytes)
//   - Backtrace elements (N * 8 bytes)
//
// The kernel updates this buffer. To read a consistent snapshot, a client can depend on the
// following memory ordering guarantees:
//
// 1. The `Header` is initialized before the buffer is shared with userspace.
//    `Header.version` should be 1.
//    `Header.event_dropped` should be read first with `memory_order_relaxed`.
// 2. For each record, the backtrace elements are written before the `backtrace_size`.
//    `backtrace_size` atomic must be read with `memory_order_acquire` to ensure visibility on
//    backtrace elements.
// 3. The `Counters` are incremented and decremented atomically without ordering guarantees.
//
// A client should first read the Header atomics, and the iterate through records reading
// `backtrace_size` first, until it finds a size of 0, which marks the end of the profile data.
namespace heap_profile {

// Header struct placed at the beginning of the heap profile buffer.
// This is exposed to the userspace.
struct Header {
  // Memory profile format version. Nonzero.
  int32_t version;
  // >0 when events where dropped because of an overflow.
  std::atomic<int32_t> event_dropped;
};

// Aggregated statistics for a given allocation backtrace.
// This is exposed to the userspace.
//
// Counters are updated without ordering or visibility guarantees.
struct Counters {
  std::atomic<uint64_t> live_count;
  std::atomic<uint64_t> live_bytes;
  std::atomic<uint64_t> total_count;
  std::atomic<uint64_t> total_bytes;

  void allocate(uint64_t size) {
    total_bytes.fetch_add(size, std::memory_order_relaxed);
    total_count.fetch_add(1, std::memory_order_relaxed);
    live_bytes.fetch_add(size, std::memory_order_relaxed);
    live_count.fetch_add(1, std::memory_order_relaxed);
  }

  void deallocate(uint64_t size) {
    live_bytes.fetch_sub(size, std::memory_order_relaxed);
    live_count.fetch_sub(1, std::memory_order_relaxed);
  }
};

// Hashtable value, stored in the heap profile buffer.
// This is exposed to the userspace.
struct Value {
  Counters counters;
  std::atomic<int64_t> backtrace_size;
};

// Page-aligned bump allocator with allocation, and offset-based data access.
template <size_t Length>
class Buffer {
 public:
  // Returns a span pointing at the whole buffer.
  std::span<const uint8_t> data() { return {data_, Length}; }

  // Allocates a block of memory large enough to hold `count` objects of type `T`.
  // The memory is initialized to zero. Returns nullopt when there is not enough space available.
  template <typename T>
  std::optional<std::span<T>> allocate(size_t count) {
    // Ensure no padding is ever required.
    static_assert(sizeof(T) % sizeof(uint64_t) == 0);
    static_assert(sizeof(uint64_t) % alignof(T) == 0);
    if (size_ + (count * sizeof(T)) > Length) {
      return {};
    }
    if (size_ % alignof(T) != 0) {
      // This should never happen because all structures size are multiple of 8 bytes.
      return {};
    }
    const size_t position = size_;
    size_ += count * sizeof(T);
    return at_offset<T>(position, count);
  }

  // Extends the used part of the buffer with the given span of objects, and returns a span pointing
  // a the copy. Returns nullopt when there is not enough space available.
  template <typename T>
  std::optional<std::span<T>> append(std::span<const T> value) {
    auto dst = allocate<T>(value.size());
    if (dst) {
      std::copy(value.begin(), value.end(), dst.value().begin());
    }
    return dst;
  }

 private:
  // Returns the offset of a given reference relative to the beginning of the buffer.
  template <typename T>
  size_t offset_for(const T& value) {
    return reinterpret_cast<const uint8_t*>(&value) - data_;
  }

  // Returns the span of `count` object of type `T` placed at the specified offset, or nullopt if
  // the span is not located inside the used buffer.
  template <typename T>
  std::optional<std::span<T>> at_offset(size_t offset, size_t count) {
    if (offset % alignof(T) != 0) {
      // This should never happen because all structures size are multiple of 8 bytes,
      // and the caller is not supposed to pass a non-aligned offset.
      return {};
    }
    if (offset >= size_ || offset + (sizeof(T) * count) > size_) {
      return {};
    }
    return {{reinterpret_cast<T*>(data_ + offset), count}};
  }

  uint8_t data_[Length] __ALIGNED(kPageSize) = {};
  size_t size_ = 0;
};

// Fixed-size lookup table tha links code path to its memory usage statistics.
//
// Template arguments:
//  BufferLength: length of the buffer holding `Header`, `Value` and backtraces.
//    Must be a multiple of kPageSize.
//  Capacity: the maximum number of 'Entry' that can be stored in the
//    hashmap.
//  BucketCount: number of bucket for the hashmap. Must be smaller than
//    EntryCapacity.
template <size_t BufferLength, size_t Capacity, size_t BucketCount>
class HeapProfileMap {
 public:
  HeapProfileMap() : header_(buffer_.template allocate<Header>(1).value()[0]) {
    header_.version = 1;
  }
  ~HeapProfileMap() { backtrace_to_values_.clear_unsafe(); }

  // Gets or creates `Counters` for a given allocation backtrace.
  // Upon success, returns a pointer to the `Counters` and the offset for that instance to be used
  // with `try_by_handle` for future retrieval.
  // When the capacity is reached, the creation fails and {nullptr, 0} is returned.
  std::tuple<Counters*, size_t> try_get(std::span<const zx_vaddr_t> backtrace) {
    Entry* entry = try_get_entry(backtrace);
    if (!entry) {
      return {nullptr, 0};
    }
    // The handle is the index in `entries_` + 1.
    // 0 is not a valid handle.
    return {&entry->value->counters, entry - entries_.data() + 1};
  }

  // Pointer to the buffer holding a live memory profile exposed to userspace.
  // The buffer is page aligned.
  std::span<const uint8_t> data() { return buffer_.data(); }

  // Increases the count of event dropped.
  void event_dropped() {
    // The increment is `memory_order_relaxed` because the reader does not require visibility
    // of other writes. It only needs to eventually see a non-zero value to fail early.
    header_.event_dropped.fetch_add(1, std::memory_order_relaxed);
  }

  // Returns a pointer to the counters at the given offset, or nullptr if
  // the offset overflows.
  Counters* try_by_handle(size_t handle) {
    if (handle == 0 || handle > entries_size_) {
      return nullptr;
    }
    return &entries_[handle - 1].value->counters;
  }

 private:
  // Hashtable entry binding the key to the value.
  using KeyType = std::span<const zx_vaddr_t>;

  struct Entry : public fbl::SinglyLinkedListable<Entry*, fbl::NodeOptions::AllowClearUnsafe> {
    Value* value;
    KeyType backtrace;

    // Trait implementation for fbl::HashTable
    static KeyType GetKey(const Entry& entry) { return entry.backtrace; }
    static bool EqualTo(const KeyType& a, const KeyType& b) {
      return std::equal(a.begin(), a.end(), b.begin(), b.end());
    }
    static size_t GetHash(const KeyType& data) {
      size_t result = 0;
      for (auto value : data) {
        result ^= value + 0x9e3779b9 + (result << 6) + (result >> 2);
      }
      return result;
    }
  };

  Entry* try_get_entry(std::span<const zx_vaddr_t> backtrace) {
    auto it = backtrace_to_values_.find(backtrace);
    if (it != backtrace_to_values_.end()) {
      return &*it;
    }

    // Add a new Value and backtrace to the buffer.
    auto stored_value = buffer_.template allocate<Value>(1);
    if (!stored_value) {
      return nullptr;  // Value buffer is full.
    }
    auto stored_backtrace = buffer_.append(backtrace);
    if (!stored_backtrace) {
      return nullptr;  // value buffer is full.
    }
    // `backtrace_size` is written with `memory_order_release` to ensure that the
    // backtrace elements are visible to any reader that acquires this value.
    stored_value.value()[0].backtrace_size.store(backtrace.size(), std::memory_order_release);

    // Add a new entry to the hashtable.
    if (entries_size_ >= entries_.size()) {
      return nullptr;  // Entry array is full.
    }
    Entry& entry = entries_[entries_size_++];
    entry.backtrace = stored_backtrace.value();
    entry.value = stored_value.value().data();
    backtrace_to_values_.insert(&entry);

    return &entry;
  }
  // Buffers shared with userspace that holds the heap profile.
  // It starts with one `Header` followed by a sequence of `(Value, backtrace)`.
  // the backtrace being an array of zx_vaddr_t for size `Value::backtrace_size`.
  Buffer<BufferLength> buffer_ = {};

  // Reference to the header placed at the beginning of the value `buffer_`.
  Header& header_;

  // Entries of the hashtable.
  std::array<Entry, Capacity> entries_ = {};
  // Current number of element in `entries_`.
  size_t entries_size_ = 0;

  // Buckets of Hashtable from backtrace to counters.
  // Either nullptr when the bucket is empty, or points at an element of `entries_`.
  fbl::HashTable<KeyType, Entry*, fbl::SinglyLinkedList<Entry*>, size_t, BucketCount,
                 /*KeyTrait*/ Entry>
      backtrace_to_values_;
};
}  // namespace heap_profile

#endif  // ZIRCON_KERNEL_LIB_HEAP_PROFILE_INCLUDE_LIB_HEAP_PROFILE_H_
