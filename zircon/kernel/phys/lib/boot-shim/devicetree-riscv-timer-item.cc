// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {

devicetree::ScanState RiscvDevicetreeTimerItem::OnNode(const devicetree::NodePath& path,
                                                       const devicetree::PropertyDecoder& decoder) {
  if (path == "/") {
    return devicetree::ScanState::kActive;
  }

  if (path == "/cpus") {
    auto freq = decoder.FindProperty("timebase-frequency");
    if (freq) {
      if (auto freq_val = freq->AsUint32()) {
        set_payload(zbi_dcfg_riscv_generic_timer_driver_t{
            .freq_hz = *freq_val,
        });
      }
    }
    return devicetree::ScanState::kDone;
  }

  return devicetree::ScanState::kDoneWithSubtree;
}

}  // namespace boot_shim
