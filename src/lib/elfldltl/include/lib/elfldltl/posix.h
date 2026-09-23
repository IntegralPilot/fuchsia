// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_POSIX_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_POSIX_H_

#include <string.h>

#include <string_view>

#include "internal/diagnostics-printf.h"

namespace elfldltl {

// PosixError is used to describe an errno value for diagnostics, see the
// diagnostics.h API describing SystemError where this type is most often
// useful.
struct PosixError {
  constexpr int operator*() const { return errnum; }

  constexpr bool operator==(PosixError other) const { return errnum == other.errnum; }
  constexpr bool operator!=(PosixError other) const { return !(*this == other); }

  const char* c_str() const { return strerror(errnum); }
  std::string_view str() const { return c_str(); }

  int errnum;
};

// This allows PosixError to be correctly printed by PrintfDiagnosticsReport.
// See internal/diagnostics-printf.h
template <>
struct internal::PrintfType<PosixError> : public internal::PrintfType<const char*> {
  static auto Arguments(PosixError err) {
    return internal::PrintfType<const char*>::Arguments(err.c_str());
  }
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_POSIX_H_
