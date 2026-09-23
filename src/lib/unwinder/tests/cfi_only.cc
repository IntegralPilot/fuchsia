// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <functional>

namespace unwinder {

int CfiOnly(std::function<int()> next) {
  // Add different values to avoid identical code folding during LTO.
  return next() + 1;
}

}  // namespace unwinder
