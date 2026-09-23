// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/ubsan-custom/handlers.h>
#include <stdio.h>
#include <zircon/assert.h>

#include <phys/main.h>

// This (via <lib/ubsan-custom/handlers.h>) defines the custom ubsan runtime
// for phys environments.  The ubsan:: functions below handle the details
// specific to the environment: how to print and how to panic.

ubsan::Report::Report(const char* check, const ubsan::SourceLocation& loc,  //
                      void* caller, void* frame) {
  if (loc.column == 0) {
    printf(
        "%s: %s:%u: *** "
        "UndefinedBehaviorSanitizer CHECK FAILED *** %s (PC=%p, FP=%p)\n",
        ProgramName(), loc.filename, loc.line, check, caller, frame);
  } else {
    printf(
        "%s: %s:%u:%u: *** "
        "UndefinedBehaviorSanitizer CHECK FAILED *** %s (PC=%p, FP=%p)\n",
        ProgramName(), loc.filename, loc.line, loc.column, check, caller, frame);
  }
}

// **Note:** //tools/testing/tefmocheck/string_in_log_check.go matches this
// exact fragment in console logs to flag reports that should not be ignored.
// So however this message changes, make sure that this fragment remains
// identical to the precise string tefmocheck matches.
#define SUMMARY_TEXT "SUMMARY: UndefinedBehaviorSanitizer"

ubsan::Report::~Report() {
  switch (BootOptions::Get() ? BootOptions::Get()->ubsan_action : CheckFailAction::kPanic) {
    case CheckFailAction::kPanic:
      ZX_PANIC("%s: *** " SUMMARY_TEXT " ERRORS! Emergency crash! ***", ProgramName());
      break;
    case CheckFailAction::kOops:
      printf("%s: *** " SUMMARY_TEXT " ERRORS! Continuing per kernel.ubsan.panic=false... ***\n",
             ProgramName());
      break;
  }
}

// This just prefixes each line with the program name and *** to look similar
// to the header and footer messages.
void ubsan::VPrintf(const char* fmt, va_list args) {
  printf("%s: *** ", ProgramName());
  vprintf(fmt, args);
}
