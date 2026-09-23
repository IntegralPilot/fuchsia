// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdlib>

#include "ufsutil.h"

int main(int argc, char** argv) {
  ufsutil::Initialize();

  if (argc < 3) {
    ufsutil::PrintUsage();
    return EXIT_FAILURE;
  }

  std::string path = argv[1];
  zx::result dev = ufsutil::OpenDevice(path.c_str());
  if (dev.is_error()) {
    fprintf(stderr, "Failed to open device: %s\n", dev.status_string());
    return EXIT_FAILURE;
  }

  return ufsutil::RunUfsUtils(
      fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>(std::move(dev.value())), argc, argv);
}
