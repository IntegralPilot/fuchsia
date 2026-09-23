// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/protocol.h>

#include "sdio.h"

int main(int argc, const char** argv) {
  if (argc == 2) {
    if (strcmp(argv[1], "--help") == 0) {
      sdio::PrintUsage();
      return 0;
    } else if (strcmp(argv[1], "--version") == 0) {
      sdio::PrintVersion();
      return 0;
    }
  }

  if (argc < 2) {
    fprintf(stderr, "Expected more arguments\n");
    sdio::PrintUsage();
    return 1;
  }

  zx::result handle = component::Connect<fuchsia_hardware_sdio::Device>(argv[1]);
  if (handle.is_error()) {
    fprintf(stderr, "Failed to open SDIO device: %s\n", handle.status_string());
    return 1;
  }

  return sdio::RunSdioTool(sdio::SdioClient(std::move(handle.value())), argc - 2, argv + 2);
}
