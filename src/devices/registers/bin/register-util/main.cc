// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/protocol.h>

#include "register-util.h"

int main(int argc, const char** argv) {
  if (argc < 3) {
    fprintf(stderr,
            "Usage: %s /path/to/device registeroffset [registervalue]\nregisteroffset and "
            "registervalue "
            "must both be formatted in hex.\nregisteroffset is the address offset from base of "
            "MMIO.\nregistervalue is optional. if it exists, write, "
            "otherwise, read.\n",
            argv[0]);
    return 0;
  }
  zx::result channel = component::Connect<fuchsia_hardware_registers::Device>(argv[1]);
  if (channel.is_error()) {
    fprintf(stderr, "Unable to open register device due to error %s\n", channel.status_string());
    return -1;
  }
  return run(argc, argv, std::move(channel.value()));
}
