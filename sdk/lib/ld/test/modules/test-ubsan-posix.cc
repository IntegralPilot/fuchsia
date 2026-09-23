// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unistd.h>

#include "test-ubsan.h"

namespace ubsan {

int LogError(std::string_view str) {
  return static_cast<int>(write(STDERR_FILENO, str.data(), str.size()));
}

}  // namespace ubsan
