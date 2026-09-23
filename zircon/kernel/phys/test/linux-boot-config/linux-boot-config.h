// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_TEST_LINUX_BOOT_CONFIG_LINUX_BOOT_CONFIG_H_
#define ZIRCON_KERNEL_PHYS_TEST_LINUX_BOOT_CONFIG_LINUX_BOOT_CONFIG_H_

#include <lib/linux-boot-config/linux-boot-config.h>

#include <ktl/optional.h>

ktl::optional<linux_boot_config::LinuxBootConfig> GetLinuxBootConfig();

#endif  // ZIRCON_KERNEL_PHYS_TEST_LINUX_BOOT_CONFIG_LINUX_BOOT_CONFIG_H_
