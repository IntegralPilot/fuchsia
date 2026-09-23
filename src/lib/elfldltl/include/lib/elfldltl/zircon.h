// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ZIRCON_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ZIRCON_H_

#include <zircon/status.h>

#include <string_view>

#include "internal/diagnostics-printf.h"

namespace elfldltl {

// ZirconError is used to describe an zx_status_t value for diagnostics, see
// the diagnostics.h API describing SystemError where this type is most often
// useful.
struct ZirconError {
  constexpr zx_status_t operator*() const { return status; }

  constexpr bool operator==(ZirconError other) const { return status == other.status; }
  constexpr bool operator!=(ZirconError other) const { return !(*this == other); }

  const char* c_str() const { return zx_status_get_string(status); }
  std::string_view str() const { return c_str(); }

  zx_status_t status;
};

// This allows ZirconError to be correctly printed by PrintfDiagnosticsReport.
// See internal/diagnostics-printf.h
template <>
struct internal::PrintfType<ZirconError> : public internal::PrintfType<const char*> {
  static auto Arguments(ZirconError err) {
    return internal::PrintfType<const char*>::Arguments(err.c_str());
  }
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ZIRCON_H_
