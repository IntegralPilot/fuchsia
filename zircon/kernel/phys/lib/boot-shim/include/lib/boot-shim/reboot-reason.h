// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_REBOOT_REASON_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_REBOOT_REASON_H_

#include <lib/zbi-format/reboot.h>
#include <stdio.h>

#include "boot-properties.h"
#include "item-base.h"

namespace boot_shim {

// Best effort translation from boot parameters (Bootconfig or cmdline) to ZBI Item.
class RebootReasonItem
    : public boot_shim::SingleOptionalItem<zbi_hw_reboot_reason_t, ZBI_TYPE_HW_REBOOT_REASON> {
 public:
  void Init(const BootProperties& properties, const char* shim_name, FILE* log = stdout);
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_REBOOT_REASON_H_
