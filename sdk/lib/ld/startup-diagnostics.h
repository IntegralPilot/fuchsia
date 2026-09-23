// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_STARTUP_DIAGNOSTICS_H_
#define LIB_LD_STARTUP_DIAGNOSTICS_H_

#include <lib/ld/diagnostics.h>

#include <cassert>
#include <cstdarg>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>

namespace [[gnu::visibility("hidden")]] ld {

// This is declared in the OS-specific header (posix.h or zircon.h).
struct StartupData;

// This is the version of the elfldltl::DiagnosticsFlags API always used in the
// startup dynamic linker: allow warnings; keep going after errors.  Before
// beginning relocation and before handing off control, the diagnostics
// object's error count will be checked to bail out safely after doing as much
// work as possible to report all the detailed errors that can be found.
struct StartupDiagnosticsFlags {
  [[no_unique_address]] elfldltl::FixedBool<true, 0> multiple_errors;
  [[no_unique_address]] elfldltl::FixedBool<false, 1> warnings_are_errors;
  [[no_unique_address]] elfldltl::FixedBool<false, 2> extra_checking;
};

// This provides the Report callable object for Diagnostics.  It uses the
// printf engine to ultimately call StartupMessage with the captured
// StartupData reference.
class DiagnosticsReport : public ModuleDiagnosticsPrintfReportBase<DiagnosticsReport> {
 public:
  constexpr explicit DiagnosticsReport(StartupData& startup) : startup_(startup) {}

  // ModuleDiagnosticsPrintfReportBase calls this like it's printf.
  void Printf(const char* format, ...) const;

  // That's just the varargs wrapper for this.
  void Printf(const char* format, va_list args) const;

  // StartupModule is a typedef in the OS-specific header, so each one defines
  // the one explicit specialization that will be used.  This can't just use a
  // simple non-template forward declaration (as with StartupData) because
  // StartupModule is a typedef and not a class or struct.
  template <class StartupModule>
  void ReportModuleLoaded(const StartupModule& module) const;

 private:
  StartupData& startup_;
};

// This is the main Diagnostics object for the startup dynamic linker.  It
// holds only the current-module state, so it can be constructed ephemerally.
struct Diagnostics : public elfldltl::Diagnostics<DiagnosticsReport, StartupDiagnosticsFlags> {
 public:
  using Base = elfldltl::Diagnostics<DiagnosticsReport, StartupDiagnosticsFlags>;

#if __has_feature(undefined_behavior_sanitizer) || LD_UBSAN
  static DiagnosticsReport* gReport_;
#else
  static constexpr auto gReport_ = std::ignore;
#endif

  Diagnostics(const Diagnostics&) = delete;
  Diagnostics(Diagnostics&&) = delete;

  constexpr explicit Diagnostics(StartupData& startup) : Base{DiagnosticsReport{startup}} {
    gReport_ = &report();
  }
};

// This is called before proceeding from loading to relocation / linking, and
// then again when proceeding from final cleanup to transferring control.
void CheckErrors(Diagnostics& diag);

}  // namespace ld

#endif  // LIB_LD_STARTUP_DIAGNOSTICS_H_
