// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_SYMBOLIZER_MARKUP_INCLUDE_LIB_SYMBOLIZER_MARKUP_LINE_BUFFERED_SINK_H_
#define SRC_LIB_SYMBOLIZER_MARKUP_INCLUDE_LIB_SYMBOLIZER_MARKUP_LINE_BUFFERED_SINK_H_

#include <array>
#include <cassert>
#include <span>
#include <string_view>

namespace symbolizer_markup {

// symbolizer_markup::LineBuffered<Size>::Sink wraps another Sink and buffers.
// The Sink class is a template within a template so that its template
// parameter can be deduced, as in:
//
//   `symbolizer_markup::LineBuffered<kBufSz>::Sink sink{inner_sink};`
//
// The inner sink is any callable object as taken by symbolizer_markup::Writer,
// and the wrapped object has the same callable signature.
//
// The outer template's parameter gives the fixed size of a buffer inside the
// Sink object.  The inner sink is called with whole lines including '\n' at
// the end, or with a full buffer that's a partial line because the Writer
// produced a line longer than the buffer size (or didn't finish the line
// before the Sink object was destroyed).

template <size_t BufferSize>
struct LineBuffered {
  static constexpr size_t kBufferSize = BufferSize;

  template <typename LineSink>
  class Sink {
   public:
    constexpr Sink() = default;

    constexpr explicit Sink(LineSink line_sink) : line_sink_{std::move(line_sink)} {}

    void operator()(std::string_view str) {
      std::span left = std::span{buffer_}.subspan(used_);
      assert(!left.empty());  // Previous call should have flushed if full.
      while (!str.empty()) {
        size_t n = str.copy(left.data(), left.size());
        assert(n > 0);
        used_ += n;
        left = left.subspan(n);
        const char last = str.back();
        str.remove_prefix(n);
        if (left.empty() || last == '\n') {
          // The buffer is full, or the string ended with a newline.  We know
          // the markup generator always makes a separate call for a newline,
          // so we don't need to search the whole string it passed.
          std::string_view chunk{buffer_.data(), buffer_.size() - left.size()};
          used_ = 0;
          left = buffer_;
          line_sink_(chunk);
        }
      }
      assert(!left.empty());  // Last iteration should have flushed if empty.
    }

    ~Sink() {
      // The last use should have been a newline, but rather than enforce that,
      // just be sure to flush.
      std::string_view chunk{buffer_.data(), used_};
      if (!chunk.empty()) {
        line_sink_(chunk);
      }
    }

   private:
    [[no_unique_address]] LineSink line_sink_;
    std::array<char, kBufferSize> buffer_;
    size_t used_ = 0;
  };
};

}  // namespace symbolizer_markup

#endif  // SRC_LIB_SYMBOLIZER_MARKUP_INCLUDE_LIB_SYMBOLIZER_MARKUP_LINE_BUFFERED_SINK_H_
