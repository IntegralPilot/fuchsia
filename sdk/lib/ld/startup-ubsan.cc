// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ubsan-custom/handlers.h>

#include "startup-diagnostics.h"

// This is set by the ld::Diagnostics constructor, so it should be set fairly
// early in StartLd.  If there is a ubsan failure report before it's set, it
// will just be silent a crash.
ld::DiagnosticsReport* ld::Diagnostics::gReport_;

namespace ubsan {
namespace {

constexpr const char* ProgramName() { return ld::abi::Abi<>::kSoname.c_str(); }

}  // namespace

// This (via <lib/ubsan-custom/handlers.h>) defines the custom ubsan runtime
// for the startup dynamic linker.  The ubsan:: functions below handle the
// details specific to the environment: how to print and how to panic.

Report::Report(const char* check, const SourceLocation& loc,  //
               void* caller, void* frame) {
  auto* report = ld::Diagnostics::gReport_;
  if (!report) {
    // Just crash immediately if no messages will get out anyway.
    __builtin_trap();
    return;
  }
  if (loc.column == 0) {
    report->Printf(
        "%s: %s:%u: *** "
        "UndefinedBehaviorSanitizer CHECK FAILED *** %s PC {{{pc:%p}}} FP %p",
        ProgramName(), loc.filename, loc.line, check, caller, frame);
  } else {
    report->Printf(
        "%s: %s:%u:%u: *** "
        "UndefinedBehaviorSanitizer CHECK FAILED *** %s PC {{{pc:%p}}} FP %p",
        ProgramName(), loc.filename, loc.line, loc.column, check, caller, frame);
  }
}

// **Note:** //tools/testing/tefmocheck/string_in_log_check.go matches this
// exact fragment in console logs to flag reports that should not be ignored.
// So however this message changes, make sure that this fragment remains
// identical to the precise string tefmocheck matches.
#define SUMMARY_TEXT "SUMMARY: UndefinedBehaviorSanitizer"

Report::~Report() {
  // The constructor crashes if gReport_ isn't set, so it always will be here.
  ld::Diagnostics::gReport_->Printf(  //
      "%s: *** " SUMMARY_TEXT " ERRORS! Emergency crash! ***", ProgramName());
  __builtin_trap();
}

void VPrintf(const char* fmt, va_list args) {
  if (auto* report = ld::Diagnostics::gReport_) {
    // It's not easy to prefix with "%s: *** " and ProgramName() in a single
    // formatted line, so these detail lines won't be marked that way.  But the
    // header and footer lines will be.
    report->Printf(fmt, args);
  }
}

}  // namespace ubsan
