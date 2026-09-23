// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_BIN_UFSUTIL_UFSUTIL_H_
#define SRC_DEVICES_BLOCK_BIN_UFSUTIL_UFSUTIL_H_

#include <fidl/fuchsia.hardware.ufs/cpp/wire.h>

namespace ufsutil {

int RunUfsUtils(fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs> client, int argc, char** argv);

zx::result<fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>> OpenDevice(const char* dev);

void PrintUsage();
void Initialize();
}  // namespace ufsutil

#endif  // SRC_DEVICES_BLOCK_BIN_UFSUTIL_UFSUTIL_H_
