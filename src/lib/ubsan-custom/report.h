// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef SRC_LIB_UBSAN_CUSTOM_REPORT_H_
#define SRC_LIB_UBSAN_CUSTOM_REPORT_H_

#include <stdarg.h>

#include "types.h"

// This file declares the interfaces that must be supplied by the embedder.
// The ubsan-handlers.h implementations use these things to report specific
// kinds of check failures.
//
// This header should only actually be included indirectly via ubsan-handlers.h
// but is separate to isolate the embedder API contract for readability.
//
// There are three functions (Report constructor, Report destructor, VPrintf)
// declared here as `inline` but not defined in this file.  This means the
// compiler will warn if the embedder does not define all of these functions
// all in the same translation unit where ubsan-handlers.h is included:
//
// ```
// ubsan::Report::Report(...) { ... }
// ubsan::Report::~Report() { ... }
// void ubsan::VPrintf(const char* fmt, ...) { ... }
// ```

namespace [[gnu::visibility("hidden")]] ubsan {

// Each ubsan handler starts by creating a ubsan::Report object to indicate
// that a report is commencing.  The constructor arguments give the name of the
// failing check, source location details, and the PC and FP of the check's
// call site.  The constructor implementation is expected to log an opening
// message with that information presented however it chooses, but presumed to
// be in whole lines.  It then uses ubsan::Printf to print additional
// information about the specific check, usually making one call per line with
// "\n" ending the format string.  Finally the Report object is destroyed when
// there are no more details to describe.  The destructor implementation may or
// may not return.  Usually check failures cause an immediate panic, but the
// implementation can choose to just return and let execution continue, where
// it might hit more check failures and report more details before crashing.
struct Report {
  Report() = delete;
  Report(const Report&) = delete;

  // The check string is a sentence or thereabouts usually with no punctuation.
  // The default arguments are evaluated in the context of the ubsan handler
  // function itself (which cannot be inlined), so they should always indicate
  // the actual call site for the specific ubsan check failure and might be
  // used for deduplication or log-throttling, etc.
  inline explicit Report(const char* check, const SourceLocation& loc,
                         void* caller = __builtin_return_address(0),
                         void* frame = __builtin_frame_address(0));

  //
  inline ~Report();
};

// This must be defined to function that is called like vprintf.
// It will be used by handlers.
[[gnu::format(printf, 1, 0)]] inline void VPrintf(const char* fmt, va_list args);

// This is is used by the handlers directly, and just calls VPrintf.
// The Report method implementations can use this too if it's convenient.
[[gnu::format(printf, 1, 2)]] inline void Printf(const char* fmt, ...) {
  va_list args;
  va_start(args, fmt);
  VPrintf(fmt, args);
  va_end(args);
}

}  // namespace ubsan

#endif  // SRC_LIB_UBSAN_CUSTOM_REPORT_H_
