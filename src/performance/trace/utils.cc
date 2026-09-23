// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace/utils.h"

#include <errno.h>
#include <lib/syslog/cpp/macros.h>
#include <string.h>

#include <fstream>
#include <string>
#include <utility>

namespace tracing {

OptionStatus ParseBooleanOption(const fxl::CommandLine& command_line, const char* name,
                                bool* out_value) {
  std::string arg;
  bool have_option = command_line.GetOptionValue(std::string_view(name), &arg);

  if (!have_option) {
    return OptionStatus::NOT_PRESENT;
  }

  if (arg == "" || arg == "true") {
    *out_value = true;
  } else if (arg == "false") {
    *out_value = false;
  } else {
    FX_LOGS(ERROR) << "Bad value for --" << name << " option, pass true or false";
    return OptionStatus::ERROR;
  }

  return OptionStatus::PRESENT;
}

std::unique_ptr<std::ofstream> OpenOutputStream(const std::string& output_file_name) {
  std::unique_ptr<std::ofstream> out_stream;
  auto ofstream =
      std::make_unique<std::ofstream>(output_file_name, std::ios_base::out | std::ios_base::trunc);
  if (ofstream->is_open()) {
    out_stream = std::move(ofstream);
  }
  return out_stream;
}

}  // namespace tracing
