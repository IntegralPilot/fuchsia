// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reverser.h"

namespace reverser {
/// Return the input string with its characters reversed.
std::string reverse_string(std::string_view input) {
  std::string output;
  output.reserve(input.length());
  for (auto it = input.rbegin(); it != input.rend(); ++it) {
    output.push_back(*it);
  }
  return output;
}
}  // namespace reverser
