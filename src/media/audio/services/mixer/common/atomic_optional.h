// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_MIXER_COMMON_ATOMIC_OPTIONAL_H_
#define SRC_MEDIA_AUDIO_SERVICES_MIXER_COMMON_ATOMIC_OPTIONAL_H_

#include <lib/syslog/cpp/macros.h>

#include <array>
#include <atomic>
#include <optional>

namespace media_audio {

// An atomic container for an optional value, where a value can be pushed to or popped from the
// container atomically.
//
// This essentially builds a single-producer multiple-consumer FIFO with a size of 1 under the hood.
// Multiple threads are allowed to `pop` a value concurrently, although only one could successfully
// retrieve the contained value. Only a single thread is allowed to `push` a value to be contained
// at a time. Both operations are lock-free and wait-free, and do not incur any dynamic memory
// allocations except for any caused by the destructor of the value type `T`.
template <typename T>
class AtomicOptional {
 public:
  AtomicOptional() = default;
  AtomicOptional(const AtomicOptional&) = delete;
  AtomicOptional& operator=(const AtomicOptional&) = delete;
  AtomicOptional(AtomicOptional&&) = delete;
  AtomicOptional& operator=(AtomicOptional&&) = delete;

  // Pushes a new value to the container if the container is empty, and returns true. If the
  // container already had a value, immediately returns false without updating the value.
  //
  // Note that this will cause the destructor of the value type `T` to be called if the push was
  // successful. Therefore, it is advisable to only call this method from a non real-time thread for
  // complex types.
  //
  // This can only be called from a single thread.
  bool push(T value) {
    const int current_index = write_index_;
    const int next_index = (current_index + 1) % kBufferSize;
    if (next_index == read_index_) {
      // The container already has a value.
      return false;
    }
    buffer_[current_index] = std::move(value);
    // Re-validate the data in `current_index` atomically.
    ptr_buffer_[current_index] = &buffer_[current_index];
    write_index_ = next_index;
    return true;
  }

  // Pops the value if any, or returns `std::nullopt` if the container has no value.
  //
  // This can be called from multiple threads.
  std::optional<T> pop() {
    const int current_index = read_index_;
    if (current_index == write_index_) {
      // The container has no value.
      return std::nullopt;
    }
    // Invalidate the data in `current_index` atomically. This is to make sure that a concurrent
    // `pop` call with the same `read_index_` cannot read the same value again.
    auto* old_ptr = ptr_buffer_[current_index].exchange(nullptr);
    if (old_ptr != &buffer_[current_index]) {
      return std::nullopt;
    }
    read_index_ = (current_index + 1) % kBufferSize;
    return std::move(*old_ptr);
  }

 private:
  // FIFO buffer with a single slot padding to guard against concurrent read and write operations.
  //
  // When `read_index_ == write_index_`, the buffer is owned by `push` and cannot be read or written
  // by `pop`.
  // When `read_index_ != write_index_`, the buffer is owned by `pop` and cannot be read or written
  // by `push`.
  static constexpr int kBufferSize = 2;
  std::array<T, kBufferSize> buffer_;
  // An additional atomic pointer buffer to guard against concurrent read operations.
  std::array<std::atomic<T*>, kBufferSize> ptr_buffer_;

  // Atomic read and write buffer indices.
  std::atomic<int> read_index_ = 0;
  std::atomic<int> write_index_ = 0;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_MIXER_COMMON_ATOMIC_OPTIONAL_H_
