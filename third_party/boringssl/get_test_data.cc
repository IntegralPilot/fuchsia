// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

// Definition expected by boringssl test source.
std::string GetTestData(const char *path) {
  std::filesystem::path abs_path{"/pkg/data"};
  abs_path.append(path);

  std::ifstream file(abs_path);
  ZX_ASSERT_MSG(file.is_open(), "Failed to open test data at %s", path);
  std::stringstream ss;
  ss << file.rdbuf();
  return ss.str();
}
