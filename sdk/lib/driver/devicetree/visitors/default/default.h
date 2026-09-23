
// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_DEFAULT_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_DEFAULT_H_

#include <lib/driver/devicetree/visitors/multivisitor.h>

#include "lib/driver/devicetree/visitors/default/bind-property/bind-property.h"
#include "lib/driver/devicetree/visitors/default/boot-metadata/boot-metadata.h"
#include "lib/driver/devicetree/visitors/default/bti/bti.h"
#include "lib/driver/devicetree/visitors/default/driver-host/driver-host.h"
#include "lib/driver/devicetree/visitors/default/fuchsia-config/fuchsia-config.h"
#include "lib/driver/devicetree/visitors/default/mmio/mmio.h"
#include "lib/driver/devicetree/visitors/default/smc/smc.h"

namespace fdf_devicetree {

// Set of visitors to parse basic devicetree properties like bind property,
// MMIO register properties etc., of each node and publish the properties to
// |fdf_devicetree::Node|. This can be extended to include driver specific
// visitors.
//     Example:
//           DefaultVisitors<MyDriverVisitor> visitors;
//           devicetree_manager.Walk(visitors);
template <typename... AdditionalVisitors>
using DefaultVisitors =
    MultiVisitor<BindPropertyVisitor, MmioVisitor, BtiVisitor, BootMetadataVisitor, SmcVisitor,
                 DriverHostVisitor, FuchsiaConfigVisitor, AdditionalVisitors...>;

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_DEFAULT_H_
