// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_TESTING_VISITOR_TEST_HELPER_H_
#define LIB_DRIVER_DEVICETREE_TESTING_VISITOR_TEST_HELPER_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <lib/driver/devicetree/manager/manager-test-helper.h>
#include <lib/driver/devicetree/manager/test-publisher.h>
#include <lib/zx/result.h>

namespace fdf_devicetree::testing {

template <class VisitorImpl>
#if __cplusplus >= 202002l
  requires std::is_base_of_v<Visitor, VisitorImpl>
#endif
class VisitorTestHelper : public VisitorImpl, public ManagerTestHelper {
 public:
  explicit VisitorTestHelper(std::string_view dtb_path, std::string_view test_tag = "")
      : ManagerTestHelper(CreateTestPublisher()), dtb_path_(dtb_path) {
    auto blob = LoadTestBlob(dtb_path_.c_str());
    manager_ = std::make_unique<Manager>(std::move(blob));
  }

  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override {
    visit_called_ = true;
    return VisitorImpl::Visit(node, decoder);
  }

  bool has_visited() { return visit_called_; }

  zx::result<> DoPublish() { return ManagerTestHelper::DoPublish(*manager_); }

  Manager* manager() {
    if (!manager_) {
      manager_ = std::make_unique<Manager>(LoadTestBlob(dtb_path_.c_str()));
    }
    return manager_.get();
  }

  std::vector<BoardChildNode> GetBoardChildNodes(
      std::optional<std::string> name_filter = std::nullopt) {
    if (!name_filter) {
      return publisher()->GetBoardChildNodes();
    }
    std::vector<BoardChildNode> output_nodes;
    for (const auto& node : publisher()->GetBoardChildNodes()) {
      if (node.name.find(*name_filter) != std::string::npos) {
        output_nodes.push_back(node);
      }
    }
    return output_nodes;
  }

  std::vector<fuchsia_hardware_platform_bus::Node> GetPbusNodes(
      std::optional<std::string> name_filter = std::nullopt) {
    if (!name_filter) {
      return publisher()->GetPbusNodes();
    }
    std::vector<fuchsia_hardware_platform_bus::Node> output_nodes;
    for (const auto& node : publisher()->GetPbusNodes()) {
      if (node.name()->find(*name_filter) != std::string::npos) {
        output_nodes.push_back(node);
      }
    }
    return output_nodes;
  }

  std::vector<fuchsia_driver_framework::CompositeNodeSpec> GetCompositeNodeSpecs(
      std::optional<std::string> name_filter = std::nullopt) {
    if (!name_filter) {
      return publisher()->GetCompositeNodeSpecs();
    }
    std::vector<fuchsia_driver_framework::CompositeNodeSpec> output_specs;
    for (const auto& request : publisher()->GetCompositeNodeSpecs()) {
      if (request.name()->find(*name_filter) != std::string::npos) {
        output_specs.push_back(request);
      }
    }
    return output_specs;
  }

  std::vector<const Node*> GetDevicetreeNodes(
      std::optional<std::string> name_filter = std::nullopt) {
    if (!name_filter) {
      std::vector<const Node*> output_nodes;
      output_nodes.reserve(manager()->nodes().size());
      for (const auto& node : manager()->nodes()) {
        output_nodes.push_back(node.get());
      }
      return output_nodes;
    }
    std::vector<const Node*> output_nodes;
    for (const auto& node : manager()->nodes()) {
      if (node->name().find(*name_filter) != std::string::npos) {
        output_nodes.push_back(node.get());
      }
    }
    return output_nodes;
  }

 private:
  bool visit_called_ = false;
  std::unique_ptr<Manager> manager_;
  std::string dtb_path_;
};

}  // namespace fdf_devicetree::testing

#endif  // LIB_DRIVER_DEVICETREE_TESTING_VISITOR_TEST_HELPER_H_
