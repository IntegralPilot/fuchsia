// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "wrapper.h"

#include "src/__support/printf_core/printf_main.h"

namespace LIBC_NAMESPACE::printf_core {
namespace {

struct WriteHook {
  int (*write)(std::string_view str, void* hook);
  void* hook;
};

int WriteHookSink(cpp::string_view str, void* arg) {
  auto [write, hook] = *static_cast<const WriteHook*>(arg);
  return write({str.data(), str.size()}, hook);
}

constexpr cpp::string_view MaybeNewline(PrintfNewline newline) {
  if (newline == PrintfNewline::kYes) {
    return "\n";
  }
  return {};
}

}  // namespace

int PrintfImpl(int (*write)(std::string_view str, void* hook), void* hook, std::span<char> buffer,
               PrintfNewline newline, const char* format, va_list args) {
  WriteHook write_hook = {.write = write, .hook = hook};
  Writer writer = make_writer(buffer.data(), buffer.size(),
                              &overflow_write_flush_to_sink<char, WriteHookSink>, &write_hook);

  internal::ArgList arg_list{args};
  auto wrote = printf_main(&writer, format, arg_list);
  if (!wrote) [[unlikely]] {
    // Any error code is lost, but it ultimately came from the callback anyway.
    return -1;
  }

  int n = overflow_write_flush_to_sink<char, WriteHookSink>(  //
      writer.get_write_buffer(), MaybeNewline(newline), &write_hook);
  if (n < 0) [[unlikely]] {
    return -1;
  }

  return static_cast<int>(*wrote) + n;
}

}  // namespace LIBC_NAMESPACE::printf_core
