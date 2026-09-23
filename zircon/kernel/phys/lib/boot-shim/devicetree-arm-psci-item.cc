// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {

devicetree::ScanState ArmDevicetreePsciItem::HandlePsciNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  if (auto method = decoder.FindProperty("method")) {
    if (auto method_str = method->AsString()) {
      set_payload(zbi_dcfg_arm_psci_driver_t{
          .use_hvc = *method_str == "hvc",
      });
    }
  } else {
    OnError("\"method\" property missing.");
  }
  return devicetree::ScanState::kDone;
}

devicetree::ScanState ArmDevicetreePsciItem::OnNode(const devicetree::NodePath& path,
                                                    const devicetree::PropertyDecoder& decoder) {
  auto [compatibles] = decoder.FindProperties("compatible");
  if (!compatibles) {
    return devicetree::ScanState::kActive;
  }

  auto compatible_list = compatibles->AsStringList();
  if (!compatible_list) {
    return devicetree::ScanState::kActive;
  }

  if (std::find_first_of(compatible_list->begin(), compatible_list->end(),
                         kCompatibleDevices.begin(),
                         kCompatibleDevices.end()) != compatible_list->end()) {
    return HandlePsciNode(path, decoder);
  }

  return devicetree::ScanState::kActive;
}

}  // namespace boot_shim
