// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_GBL_LOADER_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_GBL_LOADER_H_

#include <efi/types.h>

#include "lib/zx/result.h"

namespace gigaboot {
// Launches embedded GBL EFI app.
zx::result<> LaunchGbl(bool stop_in_fastboot);
}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_GBL_LOADER_H_
