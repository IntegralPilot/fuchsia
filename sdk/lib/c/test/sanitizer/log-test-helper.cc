// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/sanitizer.h>

#include <string_view>

int main() {
  constexpr std::string_view kHello = "Hello sanitizer logging!";
  __sanitizer_log_write(kHello.data(), kHello.size());
  return 0;
}
