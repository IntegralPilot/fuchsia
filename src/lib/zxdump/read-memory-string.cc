// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zxdump/task.h>

#include <cinttypes>

namespace zxdump {

using namespace std::literals;

// This relies on the underlying memory access doing locality-based caching
// so that repeated fetching is not too expensive.

template <typename CharT>
fit::result<Error, std::basic_string<CharT>> Process::read_memory_basic_string(uint64_t vaddr,
                                                                               size_t limit) {
  using String = std::basic_string<CharT>;
  using StringView = std::basic_string_view<CharT>;

  String str;
  while (str.size() < limit) {
    auto result = read_memory<CharT>(vaddr, 1, ReadMemorySize::kMore);
    if (result.is_error()) {
      return result.take_error();
    }

    if (result->empty()) {
      return fit::error{Error{str.empty()  // No data at all.
                                  ? "memory elided from dump"sv
                                  : "string partially elided from dump"sv,
                              ZX_ERR_NOT_SUPPORTED}};
    }

    StringView chars{result->data(), result->size()};
    if (chars.size() > limit - str.size()) {
      chars = chars.substr(0, limit - str.size());
    }
    size_t pos = chars.find_first_of('\0');
    if (pos != StringView::npos) {
      chars = chars.substr(0, pos);
    }

    if (limit - str.size() < chars.size()) {
      break;
    }

    str += chars;

    if (pos != StringView::npos) {
      return fit::ok(std::move(str));
    }
  }

  return fit::error{Error{"unterminated string"sv, ZX_ERR_OUT_OF_RANGE}};
}

// Only these instantiations are actually defined in the library.
template fit::result<Error, std::basic_string<char>> Process::read_memory_basic_string<char>(
    uint64_t vaddr, size_t limit);
#if __cpp_lib_char8_t
template fit::result<Error, std::basic_string<char8_t>> Process::read_memory_basic_string<char8_t>(
    uint64_t vaddr, size_t limit);
#endif
template fit::result<Error, std::basic_string<char16_t>>
Process::read_memory_basic_string<char16_t>(uint64_t vaddr, size_t limit);
template fit::result<Error, std::basic_string<char32_t>>
Process::read_memory_basic_string<char32_t>(uint64_t vaddr, size_t limit);
template fit::result<Error, std::basic_string<wchar_t>> Process::read_memory_basic_string<wchar_t>(
    uint64_t vaddr, size_t limit);

}  // namespace zxdump
