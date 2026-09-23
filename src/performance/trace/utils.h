// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_UTILS_H_
#define SRC_PERFORMANCE_TRACE_UTILS_H_

#include <zircon/types.h>

#include <memory>
#include <ostream>
#include <string>

#include "src/lib/fxl/command_line.h"

namespace tracing {

// Result of ParseBooleanOption.
enum class OptionStatus {
  PRESENT,
  NOT_PRESENT,
  ERROR,
};

OptionStatus ParseBooleanOption(const fxl::CommandLine& command_line, const char* name,
                                bool* out_value);

std::unique_ptr<std::ofstream> OpenOutputStream(const std::string& output_file_name);

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_UTILS_H_
