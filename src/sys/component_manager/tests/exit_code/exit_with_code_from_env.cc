// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdlib>
#include <iostream>

#include "src/lib/fxl/strings/string_number_conversions.h"

/// This program exits with the numerical code specified in the first argument.
int main(int argc, char* argv[]) {
  const char* value = getenv("EXIT_CODE");
  if (value == nullptr) {
    std::cerr << "Missing environment variable EXIT_CODE" << std::endl;
    return 1;
  }
  std::string exit_code_str(value);
  int exit_code;
  if (!fxl::StringToNumberWithError(exit_code_str, &exit_code, fxl::Base::k10)) {
    std::cerr << "Failed to parse argument as a number: " << exit_code_str << std::endl;
    return 1;
  }
  return exit_code;
}
