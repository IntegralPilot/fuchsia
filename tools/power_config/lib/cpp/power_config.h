// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_POWER_CONFIG_LIB_CPP_POWER_CONFIG_H_
#define TOOLS_POWER_CONFIG_LIB_CPP_POWER_CONFIG_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire.h>

namespace power_config {
// Load the power configuration from a file path in the global namespace.
zx::result<fuchsia_hardware_power::ComponentPowerConfiguration> Load(const char* path);

// Load the power configuration from a specific file.
zx::result<fuchsia_hardware_power::ComponentPowerConfiguration> Load(
    fidl::ClientEnd<fuchsia_io::File> file);
}  // namespace power_config

#endif  // TOOLS_POWER_CONFIG_LIB_CPP_POWER_CONFIG_H_
