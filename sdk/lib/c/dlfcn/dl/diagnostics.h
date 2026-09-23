// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DL_DIAGNOSTICS_H_
#define LIB_DL_DIAGNOSTICS_H_

#include <lib/fit/result.h>
#include <lib/ld/diagnostics.h>

#include "error.h"

namespace dl {

// dlerror() returns a single string that's not expected to contain newlines
// for multiple errors.  There's no facility for fetching or logging warnings
// at all, nor for enabling costly extra checking at runtime.  dl::Diagnostics
// will never keep going after an error.
struct DiagnosticsFlags {
  elfldltl::FixedBool<false, 0> multiple_errors;
  elfldltl::FixedBool<false, 1> warnings_are_errors;
  elfldltl::FixedBool<false, 2> extra_checking;
};

// The Report function uses dl::Error::Printf format and store the string.  It
// has rules similar to dl::Error about being used once constructed and checked
// after use.  In turn, the Diagnostics object (below) has such rules too.
class DiagnosticsReport : public ld::ModuleDiagnosticsPrintfReportBase<DiagnosticsReport> {
 public:
  DiagnosticsReport() = default;
  DiagnosticsReport(DiagnosticsReport&&) = default;
  DiagnosticsReport& operator=(DiagnosticsReport&&) = default;

  void Printf(const char* format, ...);

  // This is used via Diagnostics::take_error(), below.  The return value is
  // convertible to any fit::result<dl::Error, ...> type.  It's an assertion
  // failure unless exactly one Diagnostics API call has been made.  This
  // object should be considered moved-from after this, but it's not an rvalue
  // overload to follow the fit::result model and not require typing std::move
  // at every invocation.
  auto take_error() { return fit::error<Error>{std::move(error_).take()}; }

  // This is used in lieu of plain fit::ok(...) to indicate the state has been
  // checked and make it safe to destroy this object.  It's an assertion
  // failure if any Diagnostics API calls have been made.  This object should
  // be considered moved-from after this, as with take_error().
  template <typename... T>
  auto ok(T&&... value) {
    error_.DisarmAndAssertUnused();
    return fit::ok(std::forward<T>(value)...);
  }

 private:
  friend class Diagnostics;

  // Report an out of memory error in the event of an allocation failure.  As
  // in `ok(...)`, this asserts no prior Diagnostics API calls have been made.
  // However, the object is not moved-from because is returning to a caller of
  // the elfldltl::Diagnostics protocol.  The false return value should
  // propagate up error paths to the owner of a dl::Diagnostics object.
  bool OutOfMemory() {
    error_.DisarmAndAssertUnused();
    error_ = Error::OutOfMemory();
    return false;
  }

  Error error_;
};

// dl::Diagnostics can be used with ld::ScopedModuleDiagnostics.  It can only
// be default-constructed, and then it must be used.  It's an ephemeral object
// that should just be constructed when needed, at the top of a context where
// no module prefix is used.  Then it should be used by reference for
// operations on the root (relevant) module with ld::ScopedModuleDiagnostics
// used to set the module prefix when operating on a dependency module.  Before
// it goes out of scope, either take_error() or ok(...) must be called.
class Diagnostics : public elfldltl::Diagnostics<DiagnosticsReport, DiagnosticsFlags> {
 public:
  using Base = elfldltl::Diagnostics<DiagnosticsReport, DiagnosticsFlags>;

  Diagnostics() : Base{DiagnosticsReport{}} {}

  Diagnostics(Diagnostics&&) = delete;

  // If something using the Diagnostics template API has returned false to
  // propagate a Diagnostics return, then this can be called to close out the
  // Diagnostics object as in DiagnosticsReport (above).
  auto take_error() { return report().take_error(); }

  // This is used in lieu of plain fit::ok(...) to indicate the state has been
  // checked and make it safe to destroy this object, as in DiagnosticsReport.
  template <typename... T>
  auto ok(T&&... value) {
    return report().ok(std::forward<T>(value)...);
  }

  // This overrides elfldltl::Diagnostics<...>::FormatWarning.  Warnings don't
  // cause an error.  They don't get stored for dlerror() to return.  There's
  // no other means of logging them.  So just ignore them.
  template <typename... Args>
  std::true_type FormatWarning(Args&&... args) {
    return {};
  }

  bool OutOfMemory(std::string_view error, size_t bytes) { return report().OutOfMemory(); }
};

}  // namespace dl

#endif  // LIB_DL_DIAGNOSTICS_H_
