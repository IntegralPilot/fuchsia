// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdlib>
#include <iostream>
#include <string>

#include "src/starnix/tests/selinux/userspace/util.h"

int main(int argc, char** argv) {
  if (argc < 2) {
    std::cerr << "Usage: " << argv[0] << " <expected_domain>" << std::endl;
    return EXIT_FAILURE;
  }

  std::string expected_domain = argv[argc - 1];
  auto current_domain = ReadTaskAttr("current");

  if (current_domain.is_error()) {
    std::cerr << "Failed to read current domain: " << current_domain.error_value() << std::endl;
    return EXIT_FAILURE;
  }

  if (current_domain.value() == expected_domain) {
    return EXIT_SUCCESS;
  } else {
    std::cerr << "Domain mismatch. Expected: " << expected_domain
              << ", Got: " << current_domain.value() << std::endl;
    return EXIT_FAILURE;
  }
}
