// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/log-posix.h"

#include <unistd.h>

namespace ld {

int Log::operator()(std::string_view str) const {
  return static_cast<int>(write(STDERR_FILENO, str.data(), str.size()));
}

}  // namespace ld
