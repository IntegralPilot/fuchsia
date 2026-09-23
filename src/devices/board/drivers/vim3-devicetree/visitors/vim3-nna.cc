// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vim3-nna.h"

namespace vim3_dt {

zx::result<> Vim3NnaVisitor::DriverVisit(fdf_devicetree::Node& node,
                                         const devicetree::PropertyDecoder& decoder) {
  const uint64_t s_external_sram_phys_base = 0xFF000000;  // A311D_NNA_SRAM_BASE

  fuchsia_hardware_platform_bus::Metadata nna_metadata = {
      {.id = "0",
       .data = std::vector<uint8_t>(reinterpret_cast<const uint8_t*>(&s_external_sram_phys_base),
                                    reinterpret_cast<const uint8_t*>(&s_external_sram_phys_base) +
                                        sizeof(s_external_sram_phys_base))}};

  node.AddMetadata(nna_metadata);

  return zx::ok();
}

}  // namespace vim3_dt
