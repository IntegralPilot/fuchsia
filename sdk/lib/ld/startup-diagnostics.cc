// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "startup-diagnostics.h"

#include <cstdarg>

#include "stdio/printf_core/wrapper.h"

#ifdef __Fuchsia__
#include "zircon.h"
#else
#include "posix.h"
#endif

namespace ld {

void DiagnosticsReport::Printf(const char* format, ...) const {
  va_list args;
  va_start(args, format);
  Printf(format, args);
  va_end(args);
}

// The formatted message from elfldltl::PrintfDiagnosticsReport should be a
// single line with no newline, so append one.
void DiagnosticsReport::Printf(const char* format, va_list args) const {
  using LIBC_NAMESPACE::printf_core::Printf;
  using LIBC_NAMESPACE::printf_core::PrintfNewline;
  Printf<Log::kBufferSize, PrintfNewline::kYes>(startup_.log, format, args);
}

template <>
void DiagnosticsReport::ReportModuleLoaded<StartupModule>(const StartupModule& module) const {
  if (startup_.ld_debug) {
    ModuleSymbolizerContext<Log::kBufferSize>(startup_.log, module);
  }
}

void CheckErrors(Diagnostics& diag) {
  if (diag.errors() == 0) [[likely]] {
    return;
  }

  diag.report()("startup dynamic linking failed with", diag.errors(), " errors and",
                diag.warnings(), " warnings");
  __builtin_trap();
}

}  // namespace ld
