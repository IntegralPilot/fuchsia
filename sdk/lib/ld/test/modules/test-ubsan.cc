// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-ubsan.h"

#include <lib/ubsan-custom/handlers.h>
#include <stdarg.h>

#include "stdio/printf_core/wrapper.h"

// This (via <lib/ubsan-custom/handlers.h>) defines the custom ubsan runtime
// for the startup dynamic linker.  The ubsan:: functions below handle the
// details specific to the environment: how to print and how to panic.

namespace ubsan {

constexpr size_t kBufferSize = 128;

Report::Report(const char* check, const SourceLocation& loc,  //
               void* caller, void* frame) {
  if (loc.column == 0) {
    Printf(
        "%s:%u: *** "
        "UndefinedBehaviorSanitizer CHECK FAILED *** %s PC {{{pc:%p}}} FP %p",
        loc.filename, loc.line, check, caller, frame);
  } else {
    Printf(
        "%s:%u:%u: *** "
        "UndefinedBehaviorSanitizer CHECK FAILED *** %s PC {{{pc:%p}}} FP %p",
        loc.filename, loc.line, loc.column, check, caller, frame);
  }
}

// **Note:** //tools/testing/tefmocheck/string_in_log_check.go matches this
// exact fragment in console logs to flag reports that should not be ignored.
// So however this message changes, make sure that this fragment remains
// identical to the precise string tefmocheck matches.
#define SUMMARY_TEXT "SUMMARY: UndefinedBehaviorSanitizer"

Report::~Report() {
  Printf("*** " SUMMARY_TEXT " ERRORS! Emergency crash! ***");
  __builtin_trap();
}

void VPrintf(const char* fmt, va_list args) {
  LIBC_NAMESPACE::printf_core::Printf<                                 //
      kBufferSize, LIBC_NAMESPACE::printf_core::PrintfNewline::kYes>(  //
      LogError, fmt, args);
}

}  // namespace ubsan
