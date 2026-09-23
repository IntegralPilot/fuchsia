// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/userboot/startup.h>
#include <lib/zx/channel.h>
#include <zircon/assert.h>
#include <zircon/sanitizer.h>

#include <string_view>

namespace {

void Log(std::string_view str) { __sanitizer_log_write(str.data(), str.size()); }

}  // namespace

int main() {
  // We should have gotten a bootstrap channel with another message coming.
  zx::channel bootstrap{TakeBootstrapChannel()};
  ZX_ASSERT(bootstrap);

  // But just ignore it.
  bootstrap.reset();

  Log("Hello from userland! " BOOT_TEST_SUCCESS_STRING);
  return 0;
}
