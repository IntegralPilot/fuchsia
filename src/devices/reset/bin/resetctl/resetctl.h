// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RESET_BIN_RESETCTL_RESETCTL_H_
#define SRC_DEVICES_RESET_BIN_RESETCTL_RESETCTL_H_

#include <fidl/fuchsia.hardware.reset/cpp/wire.h>
#include <lib/zx/result.h>

namespace resetctl {

void PrintUsage(const char* binary_name);
zx::result<> Run(int argc, const char** argv,
                 fidl::ClientEnd<fuchsia_hardware_reset::Reset> client_end);

}  // namespace resetctl

#endif  // SRC_DEVICES_RESET_BIN_RESETCTL_RESETCTL_H_
