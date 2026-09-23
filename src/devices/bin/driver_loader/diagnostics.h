// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_LOADER_DIAGNOSTICS_H_
#define SRC_DEVICES_BIN_DRIVER_LOADER_DIAGNOSTICS_H_

#include <lib/elfldltl/diagnostics-ostream.h>
#include <lib/elfldltl/diagnostics.h>

#include "src/devices/lib/log/log.h"

namespace driver_loader {

struct DiagnosticsFlags {
  [[no_unique_address]] elfldltl::FixedBool<true, 0> multiple_errors;
  [[no_unique_address]] elfldltl::FixedBool<true, 1> warnings_are_errors;
  [[no_unique_address]] elfldltl::FixedBool<false, 2> extra_checking;
};

constexpr auto MakeDiagnostics() {
  constexpr auto log = [](const char* format, auto... args) { LOGF(ERROR, format, args...); };
  return elfldltl::Diagnostics{elfldltl::PrintfDiagnosticsReport(log), DiagnosticsFlags{}};
}

using Diagnostics = decltype(MakeDiagnostics());

}  // namespace driver_loader

#endif  // SRC_DEVICES_BIN_DRIVER_LOADER_DIAGNOSTICS_H_
