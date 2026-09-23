// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/crashlog.h>
#include <lib/ubsan-custom/handlers.h>
#include <platform.h>
#include <stdio.h>

namespace {

bool MustPanic() {
  return !BootOptions::Get() ||  // Anything so early it's not set yet panics.
         BootOptions::Get()->ubsan_action == CheckFailAction::kPanic;
}

}  // namespace

ubsan::Report::Report(const char* check, const ubsan::SourceLocation& loc,  //
                      void* caller, void* frame) {
  const bool must_panic = MustPanic();
  if (must_panic) {
    platform_panic_start();
  }

  fprintf(must_panic ? &stdout_panic_buffer : stdout,
          "\n"
          "*** ZIRCON KERNEL %s (caller pc: %p, stack frame: %p):\n"
          "*** ",
          must_panic ? "PANIC" : "OOPS", caller, frame);

  fprintf(&stdout_panic_buffer, "UBSAN CHECK FAILED at (%s:%d): %s\n", loc.filename, loc.line,
          check);
}

ubsan::Report::~Report() {
  if (MustPanic()) {
    fprintf(&stdout_panic_buffer, "\n");
    platform_halt(HALT_ACTION_HALT, ZirconCrashReason::Panic);
  } else {
    printf("\n");
  }
}

void ubsan::VPrintf(const char* fmt, va_list args) {
  vfprintf(MustPanic() ? &stdout_panic_buffer : stdout, fmt, args);
}
