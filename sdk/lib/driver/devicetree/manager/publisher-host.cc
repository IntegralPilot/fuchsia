// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/manager/publisher-host.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/natural_types.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include "lib/driver/devicetree/manager/zx-status-host-helper.h"

namespace fdf_devicetree {

zx::result<> PublisherHost::AddPbusNode(fuchsia_hardware_platform_bus::Node& pbus_node,
                                        std::vector<std::optional<std::string>> metadata_text,
                                        std::vector<std::optional<std::string>> power_config_text) {
  pbus_nodes_.push_back(pbus_node);
  pbus_nodes_with_metadata_.push_back({.node = std::move(pbus_node),
                                       .metadata_text = std::move(metadata_text),
                                       .power_config_text = std::move(power_config_text)});
  return zx::ok();
}

zx::result<> PublisherHost::AddBoardChildNode(BoardChildNode args) {
  board_child_nodes_.push_back(std::move(args));
  return zx::ok();
}

const std::vector<BoardChildNode>& PublisherHost::GetBoardChildNodes() {
  return board_child_nodes_;
}

const std::vector<fuchsia_hardware_platform_bus::Node>& PublisherHost::GetPbusNodes() {
  return pbus_nodes_;
}

zx::result<> PublisherHost::AddCompositeNodeSpec(
    std::string name, std::vector<fuchsia_driver_framework::ParentSpec2> parents,
    std::optional<std::string> driver_host) {
  fuchsia_driver_framework::CompositeNodeSpec spec;
  spec.name() = std::move(name);
  spec.parents2() = std::move(parents);
  composite_node_specs_.push_back(spec);
  composite_node_spec_infos_.push_back({.spec = std::move(spec), .driver_host = driver_host});
  return zx::ok();
}

zx::result<> PublisherHost::RegisterIommu(uint32_t iommu_id,
                                          fuchsia_hardware_platform_bus::Iommu iommu) {
  iommus_.insert_or_assign(iommu_id, std::move(iommu));
  return zx::ok();
}

namespace testing {

namespace {
class PublisherHostWrapper : public PublisherHost {
 public:
  PublisherHostWrapper() {
    // Initialize host logger
    logger_ = std::make_unique<fdf::Logger>("manager-test", FUCHSIA_LOG_ERROR);
    fdf::Logger::SetGlobalInstance(logger_.get());
  }
  ~PublisherHostWrapper() override { fdf::Logger::SetGlobalInstance(nullptr); }

 private:
  std::unique_ptr<fdf::Logger> logger_;
};
}  // namespace

std::unique_ptr<TestPublisher> CreateTestPublisher() {
  return std::make_unique<PublisherHostWrapper>();
}

}  // namespace testing

}  // namespace fdf_devicetree
