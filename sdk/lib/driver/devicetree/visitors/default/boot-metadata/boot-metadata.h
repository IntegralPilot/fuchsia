// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BOOT_METADATA_BOOT_METADATA_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BOOT_METADATA_BOOT_METADATA_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace fdf_devicetree {

// The |BootMetadataVisitor| populates the boot-metadata of each device tree
// node based on the "boot-metadata" property if present.
class BootMetadataVisitor : public Visitor {
 public:
  ~BootMetadataVisitor() override = default;
  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BOOT_METADATA_BOOT_METADATA_H_
