// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_STRING_BUFFER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_STRING_BUFFER_H_

#include <stdarg.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <string_view>

// The following class is excerpted from zircon/system/ulib/fbl, since we don't want to use fbl in
// bazel-based drivers.

namespace audio {

// fbl::StringBuffer: designed to resemble std::string, without allocating heap storage.
//
// The buffer is sized to hold up to N characters plus a null-terminator.
template <size_t N>
class __OWNER(char) StringBuffer final {
 public:
  // Creates an empty string buffer.
  constexpr StringBuffer() = default;

  // Creates a string buffer containing exactly one character and a null terminator.  This
  // constructor is constinit in practice so that it can be used to initialize fdio's "cwd" path
  // element without generating a dynamic initializer.
  constexpr explicit StringBuffer(char c) : length_(1), data_{c, '\0'} { static_assert(N >= 1); }

  // Releases the string buffer.
  ~StringBuffer() = default;

  // Returns a pointer to the null-terminated contents of the string.
  char* data() { return data_; }
  const char* data() const { return data_; }
  const char* c_str() const { return data_; }

  // Returns the length of the string, excluding its null terminator.
  size_t length() const { return length_; }
  size_t size() const { return length_; }

  // Clears existing data and sets the buffer to the new value, plus a null terminator.
  void Set(std::string_view data) {
    ZX_DEBUG_ASSERT(data.size() < N);
    length_ = data.length();
    memcpy(data_, data.data(), data.length());
    data_[length_] = '\0';
  }

  // Appends content to the string buffer from a string view.
  // The result is truncated if the appended content does not fit completely.
  StringBuffer& Append(std::string_view view) {
    AppendInternal(view.data(), view.length());
    return *this;
  }

  // Creates a string view backed by the string.
  // The string_view does not take ownership of the data so the string must outlast the string_view.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  operator std::string_view() const { return {data(), length()}; }

 private:
  void AppendInternal(const char* data, size_t length) {
    size_t remaining = N - length_;
    length = std::min(length, remaining);
    memcpy(data_ + length_, data, length);
    length_ += length;
    data_[length_] = 0;
  }

  size_t length_ = 0U;
  char data_[N + 1U] = {0};
};

}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_STRING_BUFFER_H_
