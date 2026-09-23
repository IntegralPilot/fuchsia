// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_DEV_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_DEV_H_

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>

#include "lib/driver/devicetree/manager/publisher.h"

namespace fdf_devicetree {

class PublisherDev : public PublisherInterface {
 public:
  PublisherDev(fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus,
               fidl::SyncClient<fuchsia_driver_framework::CompositeNodeManager>& mgr,
               fidl::SyncClient<fuchsia_driver_framework::Node>& fdf_node)
      : pbus_(pbus), mgr_(mgr), fdf_node_(fdf_node) {}

  zx::result<> AddPbusNode(fuchsia_hardware_platform_bus::Node& pbus_node,
                           std::vector<std::optional<std::string>> metadata_text = {},
                           std::vector<std::optional<std::string>> power_config_text = {}) override;

  zx::result<> AddBoardChildNode(BoardChildNode args) override;

  zx::result<> AddCompositeNodeSpec(std::string name,
                                    std::vector<fuchsia_driver_framework::ParentSpec2> parents,
                                    std::optional<std::string> driver_host) override;

  zx::result<> RegisterIommu(uint32_t iommu_id,
                             fuchsia_hardware_platform_bus::Iommu iommu) override;

 private:
  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus_;
  fidl::SyncClient<fuchsia_driver_framework::CompositeNodeManager>& mgr_;
  fidl::SyncClient<fuchsia_driver_framework::Node>& fdf_node_;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_DEV_H_
