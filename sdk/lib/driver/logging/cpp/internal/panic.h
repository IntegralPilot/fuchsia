// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_LOGGING_CPP_INTERNAL_PANIC_H_
#define LIB_DRIVER_LOGGING_CPP_INTERNAL_PANIC_H_

#include <format>

namespace fdf_internal {

class output_iterator {
 public:
  using iterator_category = std::output_iterator_tag;
  using value_type = char;
  using difference_type = std::ptrdiff_t;
  using pointer = char*;
  using reference = char&;

  explicit output_iterator(std::span<char> buffer) : buffer_(buffer) {}

  output_iterator(output_iterator&& other) noexcept
      : buffer_(other.buffer_), index_(other.index_) {}
  output_iterator& operator=(output_iterator&& other) noexcept {
    buffer_ = other.buffer_;
    index_ = other.index_;
    return *this;
  }
  output_iterator(const output_iterator& other) noexcept = default;
  output_iterator& operator=(const output_iterator& other) noexcept = delete;

  output_iterator& operator=(const char& ch) {
    buffer_[index_] = ch;
    return *this;
  }

  reference operator*() { return buffer_[index_]; }

  output_iterator& operator++() {
    index_++;
    if (index_ == buffer_.size()) {
      fwrite(buffer_.data(), 1, buffer_.size(), stderr);
      index_ = 0;
    }
    return *this;
  }

  output_iterator operator++(int) {
    auto tmp = *this;
    ++*this;
    return tmp;
  }

  size_t index() const { return index_; }

 private:
  std::span<char> buffer_;
  size_t index_ = 0;
};

inline void vpanic(std::string_view fmt, std::format_args args) {
  constexpr size_t kBufferSize = 256;
  std::array<char, kBufferSize> buffer = {};
  size_t index = std::vformat_to(output_iterator(buffer), fmt, args).index();
  if (index > 0) {
    fwrite(buffer.data(), 1, index, stderr);
  }
  __builtin_trap();
}

}  // namespace fdf_internal

#endif  // LIB_DRIVER_LOGGING_CPP_INTERNAL_PANIC_H_
