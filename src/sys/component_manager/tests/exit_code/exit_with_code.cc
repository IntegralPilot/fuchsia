// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdlib>
#include <iostream>

#include "src/lib/fxl/strings/string_number_conversions.h"

/// This program exits with the numerical code specified in the first argument.
int main(int argc, char* argv[]) {
  if (argc < 2) {
    std::cerr << "Missing argument" << std::endl;
    return 1;
  }
  std::string str(argv[1]);
  int exit_code;
  if (!fxl::StringToNumberWithError(str, &exit_code, fxl::Base::k10)) {
    std::cerr << "Failed to parse argument as a number: " << str << std::endl;
  }
  return exit_code;
}
