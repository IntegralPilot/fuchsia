// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_APPLE_PLATFORM_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_APPLE_PLATFORM_H_

#include <lib/boot-shim/devicetree.h>
#include <lib/zbi-format/board.h>

#include <algorithm>

namespace boot_shim {

class AppleDevicetreePlatformItem
    : public DevicetreeItemBase<AppleDevicetreePlatformItem, 1>,
      public SingleOptionalItem<zbi_platform_id_t, ZBI_TYPE_PLATFORM_ID> {
 public:
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder) {
    if (path == "/") {
      auto compatible =
          decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
      if (compatible &&
          std::find(compatible->begin(), compatible->end(), "apple,t8122") != compatible->end()) {
        // Generic VID/PID: there is no legacy Apple platform-board driver to bind.
        set_payload({.vid = 0, .pid = 0, .board_name = "apple-t8122"});
      }
    }
    return devicetree::ScanState::kDone;
  }
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_APPLE_PLATFORM_H_
