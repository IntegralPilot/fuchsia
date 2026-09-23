// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "lib/driver/devicetree/visitors/default/boot-metadata/boot-metadata.h"

#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/devicetree/devicetree.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

#include <cstdint>
#include <optional>

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace fdf_devicetree {

constexpr const char KBootMetadataProp[] = "boot-metadata";
constexpr const uint32_t kBootMetadataCellSize = 2;
constexpr const uint32_t kZbiTypeCellSize = 1;
constexpr const uint32_t kZbiExtraCellSize = 1;

class BootMetadataElement : public devicetree::PropEncodedArrayElement<kBootMetadataCellSize> {
 public:
  using PropEncodedArrayElement<kBootMetadataCellSize>::PropEncodedArrayElement;

  // 1st cell represents the ZBI type.
  uint32_t zbi_type() {
    std::optional<uint64_t> cell = (*this)[0];
    return static_cast<uint32_t>(*cell);
  }

  // 2nd cell represents the ZBI extra value.
  uint32_t zbi_extra() {
    std::optional<uint64_t> cell = (*this)[1];
    return static_cast<uint32_t>(*cell);
  }
};

zx::result<> BootMetadataVisitor::Visit(Node& node, const devicetree::PropertyDecoder& decoder) {
  auto boot_metadata_prop = node.properties().find(KBootMetadataProp);
  if (boot_metadata_prop == node.properties().end()) {
    return zx::ok();
  }

  auto boot_metadata_array = devicetree::PropEncodedArray<BootMetadataElement>(
      boot_metadata_prop->second.AsBytes(), kZbiTypeCellSize, kZbiExtraCellSize);
  for (uint32_t index = 0; index < boot_metadata_array.size(); index++) {
    fuchsia_hardware_platform_bus::BootMetadata metadata = {{
        .zbi_type = boot_metadata_array[index].zbi_type(),
        .zbi_extra = boot_metadata_array[index].zbi_extra(),
    }};
    fdf::debug("Boot metadata ({:#x}, {:#x}) added to node '{}'.", *metadata.zbi_type(),
               *metadata.zbi_extra(), node.name());

    node.AddBootMetadata(metadata);
  }

  return zx::ok();
}

}  // namespace fdf_devicetree
