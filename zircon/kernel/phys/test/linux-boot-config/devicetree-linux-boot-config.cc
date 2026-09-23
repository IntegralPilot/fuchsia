// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/boot-shim/devicetree.h>

#include "linux-boot-config.h"

ktl::optional<linux_boot_config::LinuxBootConfig> GetLinuxBootConfig() {
  return gDevicetreeBoot.linux_boot_config;
}
