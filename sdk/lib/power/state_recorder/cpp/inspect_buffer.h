// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_INSPECT_BUFFER_H_
#define LIB_POWER_STATE_RECORDER_CPP_INSPECT_BUFFER_H_

#include <lib/fit/function.h>
#include <lib/power/state_recorder/cpp/concepts.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <algorithm>
#include <cstdint>
#include <limits>
#include <type_traits>
#include <vector>

namespace power_observability::internal {

inline int64_t to_msecs(zx::time_boot timestamp) {
  return zx::duration(timestamp.to_timespec()).to_msecs();
}

// The logical data points stored in a TimestampedBuffer.
template <typename ValueType>
struct DataPoint {
  int64_t timestamp_ns;
  ValueType value;
};

// Simple wrapper around std::vector<ValueType> for use with TimestampedBuffer.
template <typename ValueType>
class ValueBuffer {
 public:
  explicit ValueBuffer(size_t size) : buffer_(size) {}
  ValueType Get(size_t index) const { return buffer_[index]; }
  void Set(size_t index, ValueType value) { buffer_[index] = value; }
  void Reset() { std::ranges::fill(buffer_, static_cast<ValueType>(0)); }

 private:
  std::vector<ValueType> buffer_;
};

// Stores bits in an underlying byte array, for use with TimestampedBuffer<bool>.
class BitBuffer {
 public:
  explicit BitBuffer(size_t size) : buffer_((size + 7) / 8) {}
  bool Get(size_t index) const {
    size_t byte_idx = index >> 3;
    size_t bit_idx = index & 0x7;
    return (buffer_[byte_idx] >> bit_idx) & 1;
  }
  void Set(size_t index, bool value) {
    size_t byte_idx = index >> 3;
    size_t bit_idx = index & 0x7;
    if (value) {
      buffer_[byte_idx] |= (1 << bit_idx);
    } else {
      buffer_[byte_idx] &= ~(1 << bit_idx);
    }
  }
  void Reset() { std::ranges::fill(buffer_, 0); }

 private:
  std::vector<uint8_t> buffer_;
};

// Define is_bool_enum_v such that std::underlying_type_t is never evaluated for a non-enum type.
template <typename T>
inline constexpr bool is_bool_enum_v = false;

template <typename T>
  requires std::is_enum_v<T>
inline constexpr bool is_bool_enum_v<T> = std::is_same_v<std::underlying_type_t<T>, bool>;

// Used to track buffer resets due to timestamp delta over/underflow.
struct ResetInfo {
  size_t count;
  int64_t last_reset_ns;
};

// Manages circular buffers for timestamp deltas and associated data.
template <typename ValueType>
  requires IsRecordableValueType<ValueType>
class TimestampedBuffer {
 public:
  // If ValueType is bool or an enum type with bool underlying type, use BitBuffer. Otherwise, use
  // ValueBuffer<ValueType>.
  using BufferType =
      std::conditional_t<std::is_same_v<ValueType, bool> || is_bool_enum_v<ValueType>, BitBuffer,
                         ValueBuffer<ValueType>>;

  explicit TimestampedBuffer(size_t size)
      : delta_ms_buffer_(size), data_buffer_(BufferType(size)), buffer_size_(size) {
    reset_info_.last_reset_ns = zx::msec(to_msecs(zx::clock::get_boot())).get();
  }

  // Delete move and copy constructors until we have a good reason to use them.
  TimestampedBuffer(TimestampedBuffer&& other) = delete;
  TimestampedBuffer& operator=(TimestampedBuffer&& other) = delete;
  TimestampedBuffer(const TimestampedBuffer& other) = delete;
  TimestampedBuffer& operator=(const TimestampedBuffer& other) = delete;

  // Adds an entry to the buffer, at millisecond timestamp `current_ms`.
  //
  // If the millisecond delta between the new timestamp and the most recent timestamp cannot be
  // represented as an int32_t (approximately 24.9 days), the buffer will be reset, retaining only
  // the supplied entry.
  void AddEntry(ValueType data, int64_t current_ms) {
    int32_t delta = 0;
    if (!initialized_) {
      // Initialize the buffer on the first call.
      base_ts_ms_ = current_ms;
      last_ts_ms_ = current_ms;
      initialized_ = true;
    } else {
      int64_t int64_delta = current_ms - last_ts_ms_;
      if (std::numeric_limits<int32_t>::min() <= int64_delta &&
          int64_delta <= std::numeric_limits<int32_t>::max()) {
        delta = static_cast<int32_t>(int64_delta);
      } else {
        // The timestamp delta overflowed or underflowed. Reset the buffer to its initialized state.
        std::ranges::fill(delta_ms_buffer_, 0);
        data_buffer_.Reset();
        base_ts_ms_ = current_ms;
        last_ts_ms_ = current_ms;
        write_idx_ = 0;
        count_ = 0;

        reset_info_.count += 1;
        reset_info_.last_reset_ns = zx::msec(current_ms).to_nsecs();
      }
    }
    // Update base time on wrap-around (before overwriting)
    if (count_ == buffer_size_) {
      base_ts_ms_ += delta_ms_buffer_[write_idx_];
    }
    // Store the new delta and its corresponding data
    delta_ms_buffer_[write_idx_] = delta;
    data_buffer_.Set(write_idx_, data);
    // Update state for the next entry
    last_ts_ms_ = current_ms;
    write_idx_ = (write_idx_ + 1) % buffer_size_;
    if (count_ < buffer_size_) {
      count_++;
    }
  }

  // Reconstructs each DataPoint stored in the buffer and runs `callback` on it.
  void ForEachDataPoint(fit::function<void(const DataPoint<ValueType>&)> callback) const {
    if (!initialized_ || count_ == 0) {
      return;
    }

    zx::duration current_ts = zx::msec(base_ts_ms_);
    size_t read_idx = (count_ == buffer_size_) ? write_idx_ : 0;

    for (size_t i = 0; i < count_; ++i) {
      current_ts += zx::msec(delta_ms_buffer_[read_idx]);
      ValueType value = data_buffer_.Get(read_idx);
      callback({.timestamp_ns = current_ts.to_nsecs(), .value = value});
      read_idx = (read_idx + 1) % buffer_size_;
    }
  }

  const ResetInfo& GetResetInfo() const { return reset_info_; }
  size_t GetCount() const { return count_; }
  size_t GetCapacity() const { return buffer_size_; }

 private:
  std::vector<int32_t> delta_ms_buffer_;
  BufferType data_buffer_;
  size_t buffer_size_;
  int64_t base_ts_ms_ = 0;
  int64_t last_ts_ms_ = 0;
  size_t write_idx_ = 0;
  size_t count_ = 0;
  ResetInfo reset_info_ = {.count = 0, .last_reset_ns = 0};
  bool initialized_ = false;
};

}  // namespace power_observability::internal

#endif  // LIB_POWER_STATE_RECORDER_CPP_INSPECT_BUFFER_H_
