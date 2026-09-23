// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_TEST_PUBLISHER_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_TEST_PUBLISHER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <lib/driver/devicetree/manager/publisher.h>

#include <memory>

namespace fdf_devicetree::testing {

class TestPublisher : public PublisherInterface {
 public:
  virtual const std::vector<BoardChildNode>& GetBoardChildNodes() = 0;
  virtual const std::vector<fuchsia_hardware_platform_bus::Node>& GetPbusNodes() = 0;
  virtual const std::vector<fuchsia_driver_framework::CompositeNodeSpec>&
  GetCompositeNodeSpecs() = 0;
  virtual const std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu>& GetIommus() = 0;
};

std::unique_ptr<TestPublisher> CreateTestPublisher();

}  // namespace fdf_devicetree::testing

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_TEST_PUBLISHER_H_
