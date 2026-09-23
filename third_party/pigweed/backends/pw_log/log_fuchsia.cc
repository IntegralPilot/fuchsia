// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pw_log_fuchsia/log_fuchsia.h"

#include <cstdarg>
#include <cstdio>
#include <string_view>

#include "pw_log/levels.h"
#include "pw_log_backend/log_backend.h"
#include "pw_string/string_builder.h"

namespace {

// This is an arbitrary size limit to printf logs.
constexpr size_t kPrintfBufferSize = 4096;

// Tests for pw_log_fuchsia::BaseName:
static_assert(pw_log_fuchsia::BaseName(nullptr) == nullptr);
static_assert(std::string_view(pw_log_fuchsia::BaseName("")).empty());
static_assert(std::string_view(pw_log_fuchsia::BaseName("main.cc")) == std::string_view("main.cc"));
static_assert(std::string_view(pw_log_fuchsia::BaseName("/main.cc")) ==
              std::string_view("main.cc"));
static_assert(std::string_view(pw_log_fuchsia::BaseName("../foo/bar//main.cc")) ==
              std::string_view("main.cc"));

}  // namespace

extern "C" void pw_Log(int level, const char* module_name, unsigned int flags,
                       const char* file_name, int line_number, const char* message, ...) {
  if (flags & PW_LOG_FLAG_IGNORE) {
    return;
  }

  pw::StringBuffer<kPrintfBufferSize> formatted;

  va_list args;
  va_start(args, message);
  formatted.FormatVaList(message, args);
  va_end(args);

  if (flags & PW_LOG_FLAG_USE_PRINTF) {
    printf("%s: [%s:%s:%d] %s\n", pw_log_fuchsia::LogLevelToString(level), module_name,
           pw_log_fuchsia::BaseName(file_name), line_number, formatted.c_str());
    return;
  }

  pw_log_fuchsia_impl(level, module_name, file_name, line_number, formatted.c_str());
}
