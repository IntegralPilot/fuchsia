// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_ROLLING_BUFFER_H_
#define ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_ROLLING_BUFFER_H_

/// This class provides a view into a buffer that implements a lockless multi-writer single reader
/// specialized for trace-engine.
///
/// Several design choices made here were done so to maintain compatibility with the previous
/// algorithm and current apis exposed by trace-engine. As such, this buffer may not be an ideal
/// abstraction for other use cases. Such decisions include:
///
/// - Not providing a means to commit writes: the caller must externally manage writes in flights to
///   correctly read and service the buffers.
/// - Double buffering rather than concurrent reads/writes.
/// - Exposing buffer swap functionality to the caller. Writers are responsible for ensuring that
///   buffers are correctly serviced when notified.
///
/// See the file comments in //zircon/system/ulib/trace-engine/rolling_buffer.cc for details on
/// operation. See //zircon/system/ulib/trace-engine/test/rolling_buffer_test.cc for examples on
/// usage.

#include <atomic>
#include <span>
#include <variant>

enum class BufferNumber : uint8_t {
  kZero = 0,
  kOne = 1,
};

/// The buffer maintains state using a single uint64_t atomic.
///
/// The state is split using the following bit fields:
///
///  Buffer Status, 2 bits, 0xC000'0000'0000'0000:
///      Indicates if one of the double buffers is full and requires servicing.
///      Buffer 0 is indicated by the MSB.
///
///  Wrapped Counter, 30 bits, 0x3FFF'FFFF'0000'0000:
///      Tracks the number of buffer swaps we have completed this session. Serves as an indicator of
///      which buffer is the active (writing) buffer.
///
///  Offset, 32 bits, 0x0000'0000'FFFF'FFFF:
///      Tracks the offset of the allocation pointer into the current buffer.
///
/// This class serves as an abstraction over reading and writing theses fields.
class RollingBufferState {
 public:
  RollingBufferState() = default;
  explicit RollingBufferState(uint64_t state) : state_(state) {}
  RollingBufferState(uint8_t buffer_status, uint32_t offset, uint32_t counter)
      : state_(uint64_t{offset} | (static_cast<uint64_t>(counter) << kWrappedCounterShift) |
               (static_cast<uint64_t>(buffer_status) << kBufferStatusShift)) {}
  bool operator<=>(const RollingBufferState&) const = default;

  uint32_t GetBufferOffset() const {
    return static_cast<uint32_t>((state_ & kOffsetMask) >> kOffsetShift);
  }

  bool IsBufferFilled(BufferNumber buffer_number) const {
    uint64_t buffer_to_check = buffer_number == BufferNumber::kZero ? 0b01 : 0b10;
    return (state_ & (buffer_to_check << kBufferStatusShift)) != 0;
  }

  [[nodiscard]]  // The returns a new state, it doesn't modify the existing one.
  RollingBufferState WithBufferMarkedFilled(BufferNumber buffer_number) const {
    const uint8_t desired_set_buffer = buffer_number == BufferNumber::kZero ? 0b01 : 0b10;
    return RollingBufferState{state_ | (uint64_t{desired_set_buffer} << kBufferStatusShift)};
  }

  [[nodiscard]]  // The returns a new state, it doesn't modify the existing one.
  RollingBufferState WithBufferMarkedServiced(BufferNumber buffer_number) const {
    const uint8_t desired_unset_buffer = buffer_number == BufferNumber::kZero ? 0b01 : 0b10;
    return RollingBufferState{state_ & ~(uint64_t{desired_unset_buffer} << kBufferStatusShift)};
  }

  [[nodiscard]]  // The returns a new state, it doesn't modify the existing one.
  RollingBufferState WithAllocatedBytes(uint32_t num_bytes) const {
    return RollingBufferState{state_ + num_bytes};
  }

  [[nodiscard]]  // The returns a new state, it doesn't modify the existing one.
  RollingBufferState WithOffset(uint32_t new_offset) const {
    uint64_t masked_state = state_ & ~kOffsetMask;
    return RollingBufferState{masked_state | (new_offset << kOffsetShift)};
  }

  [[nodiscard]]  // The returns a new state, it doesn't modify the existing one.
  RollingBufferState WithNextWrappedCount() const {
    uint64_t wrapped_increment = uint64_t{1} << kWrappedCounterShift;
    return RollingBufferState{state_ + wrapped_increment};
  }

  uint32_t GetWrappedCount() const {
    return static_cast<uint32_t>((state_ & kWrappedCounterMask) >> kWrappedCounterShift);
  }

  uint8_t GetBufferStatus() const {
    return static_cast<uint8_t>((state_ & kBufferStatusMask) >> kBufferStatusShift);
  }

  static BufferNumber ToBufferNumber(uint32_t wrapped_count) {
    return static_cast<BufferNumber>(wrapped_count & 1);
  }
  BufferNumber GetBufferNumber() const { return ToBufferNumber(GetWrappedCount()); }

 private:
  // We use two bits to mark that a given rolling buffer is full.
  static constexpr size_t kBufferStatusShift = 62;
  static constexpr uint64_t kBufferStatusMask = 0xC000'0000'0000'0000;

  static constexpr size_t kWrappedCounterShift = 32;
  static constexpr uint64_t kWrappedCounterMask = 0x3FFF'FFFF'0000'0000;

  static constexpr size_t kOffsetShift = 0;
  static constexpr uint64_t kOffsetMask = 0x0000'0000'FFFF'FFFF;

  uint64_t state_ = 0;
};

struct BufferFull {};
struct SwitchingAllocation {
  // The state the buffer is in when the write failed.
  uint64_t* ptr;
  RollingBufferState prev_state;
};
struct Allocation {
  // A pointer to the allocated memory.
  uint64_t* ptr;
};
using AllocationResult = std::variant<Allocation, BufferFull, SwitchingAllocation>;

template <class... Ts>
struct AllocationVisitor : Ts... {
  using Ts::operator()...;
};

class RollingBuffer {
 public:
  void SetOneshotBuffer(std::span<uint8_t> buffer) {
    rolling_buffers_[0] = buffer;
    rolling_buffers_[1] = std::span<uint8_t>{};
  }

  void SetDoubleBuffers(std::span<uint8_t> buffer0, std::span<uint8_t> buffer1) {
    rolling_buffers_[0] = buffer0;
    rolling_buffers_[1] = buffer1;
  }

  // Allocate data from the configured buffer. Thread safe.
  //
  // When we allocate data, we may get:
  // 1) A successful Allocation. We may write our data into the provided pointer.
  // 2) A successful SwitchingAllocation. We may write out data into the provided pointer. However,
  //    this also contains a notification that the previous buffer was full and requires servicing,
  //    as well as includes the state of the buffer directly before writing the record. This is
  //    guaranteed to be returned only once per wrap count.
  //    SAFETY: This notification means that the buffer has filled, but there may still be active
  //    writers in the buffer. The receiver is responsible for ensuring that all writes are complete
  //    before attempting to read or service the buffer.
  // 3) A failure with BufferFull. There's no room in either buffer, and we're waiting for someone
  //    to service the buffers so there's nothing to do.
  AllocationResult AllocRecord(size_t num_bytes);

  AllocationResult Flush();

  // Return the total number of allocated bytes in the current buffer.
  size_t BytesAllocated() const;

  // Set the allocation pointer back to the beginning of the buffer and consider it to be empty.
  void Reset();

  // Get the per-buffer size of the rolling buffers
  size_t BufferSize(BufferNumber idx) const {
    return rolling_buffers_[static_cast<uint8_t>(idx)].size_bytes();
  }

  // Get the total size of all the rolling buffers
  size_t MaxBufferSize() const {
    return rolling_buffers_[0].size_bytes() + rolling_buffers_[1].size_bytes();
  }

  RollingBufferState State() const { return state_.load(std::memory_order_relaxed); }

  // The maximum rolling buffer size in bits.
  static constexpr size_t kRollingBufferSizeBits = 32;

  // Maximum size, in bytes, of a rolling buffer.
  static constexpr size_t kMaxRollingBufferSize = 1ull << kRollingBufferSizeBits;

  /// Mark that we are done with this buffer, freeing the space for more writes.
  /// SAFETY: Caller is responsible for ensuring that there are no remaining active writers in this
  ///          buffer before calling this.
  bool SetBufferServiced(uint32_t wrapped_count);

  // Mark the currently set buffer as full.
  void SetBufferFull();

 private:
  // Our rolling double buffers.
  std::span<uint8_t> rolling_buffers_[2];

  // The underlying atomic state of our wrapped count, buffer service status, and allocation
  // pointer.
  std::atomic<RollingBufferState> state_;
  static_assert(std::atomic<RollingBufferState>::is_always_lock_free,
                "RollingBufferState doesn't fit into a supported atomic size");
};

#endif  // ZIRCON_SYSTEM_ULIB_TRACE_ENGINE_ROLLING_BUFFER_H_
