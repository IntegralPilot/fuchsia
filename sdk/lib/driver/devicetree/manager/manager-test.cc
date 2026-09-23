// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "manager.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devicetree/visitors/default/default.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/registry.h>
#include <zircon/errors.h>

#include <cstddef>
#include <memory>
#include <optional>
#include <unordered_set>
#include <utility>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <gtest/gtest.h>

#include "manager-test-helper.h"
#include "test-data/basic-properties.h"
#include "test-data/simple.h"
#include "test-publisher.h"
#include "visitor.h"

namespace fdf_devicetree {
namespace {

class ManagerTest : public testing::ManagerTestHelper, public ::testing::Test {
 public:
  ManagerTest() : ManagerTestHelper(testing::CreateTestPublisher()) {}
};

TEST_F(ManagerTest, TestFindsNodes) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  class EmptyVisitor : public Visitor {
   public:
    zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      return zx::ok();
    }
  };
  EmptyVisitor visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());
  ASSERT_EQ(3lu, manager.nodes().size());

  // Root node is always first, and has no name.
  Node* node = manager.nodes()[0].get();
  ASSERT_STREQ("dt-root", node->name().data());

  // example-device node should be next.
  node = manager.nodes()[1].get();
  ASSERT_STREQ("example-device", node->name().data());

  // another-device should be last.
  node = manager.nodes()[2].get();
  ASSERT_STREQ("another-device", node->name().data());
}

TEST_F(ManagerTest, TestPropertyCallback) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  class TestVisitor : public Visitor {
   public:
    zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      for (auto& [name, _] : node.properties()) {
        if (node.name() == "example-device") {
          auto iter = expected.find(std::string(name));
          EXPECT_NE(expected.end(), iter) << "Property " << name << " was unexpected.";
          if (iter != expected.end()) {
            expected.erase(iter);
          }
        }
      }
      return zx::ok();
    }

    std::unordered_set<std::string> expected{
        "compatible",
        "phandle",
    };
  };

  TestVisitor visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());
  EXPECT_EQ(0lu, visitor.expected.size());
}

// TODO(https://fxbug.dev/488154882): Rename the test and the referenced .dtb.
TEST_F(ManagerTest, TestPublishesSimpleNode) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  DefaultVisitors<> default_visitors;
  ASSERT_EQ(ZX_OK, manager.Walk(default_visitors).status_value());

  ASSERT_TRUE(DoPublish(manager).is_ok());
  ASSERT_EQ(0lu, publisher()->GetPbusNodes().size());
  ASSERT_EQ(2lu, publisher()->GetBoardChildNodes().size());

  ASSERT_EQ(2lu, publisher()->GetCompositeNodeSpecs().size());

  auto board_child_node_0 = publisher()->GetBoardChildNodes()[0];

  ASSERT_EQ(board_child_node_0.name, "dt-root");
  ASSERT_TRUE(!board_child_node_0.properties.empty());

  ASSERT_TRUE(testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::COMPATIBLE, "fuchsia,sample-dt"),
      },
      board_child_node_0.properties, false));

  auto board_child_node_1 = publisher()->GetBoardChildNodes()[1];

  ASSERT_NE(nullptr, strstr("example-device", board_child_node_1.name.data()));
  ASSERT_TRUE(!board_child_node_1.properties.empty());

  ASSERT_TRUE(testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::COMPATIBLE, "fuchsia,sample-device"),
      },
      board_child_node_1.properties, false));
}

TEST_F(ManagerTest, DriverVisitorTest) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/basic-properties.dtb"));

  class TestDriverVisitor final : public DriverVisitor {
   public:
    TestDriverVisitor() : DriverVisitor({"wrong-string", "fuchsia,sample-device"}) {}

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      visited = true;
      return zx::ok();
    }
    bool visited = false;
  };

  TestDriverVisitor visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());

  ASSERT_TRUE(DoPublish(manager).is_ok());
  ASSERT_TRUE(visitor.visited);
}

TEST_F(ManagerTest, TestMetadata) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/basic-properties.dtb"));

  class MetadataVisitor : public DriverVisitor {
   public:
    MetadataVisitor() : DriverVisitor({"fuchsia,sample-device"}) {}

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      auto prop = node.GetProperty<uint32_t>("device_specific_prop");
      EXPECT_TRUE(prop.is_ok()) << "Property device_specific_prop was unexpected.";
      device_specific_prop = prop.value_or(ZX_ERR_INVALID_ARGS);
      EXPECT_EQ(device_specific_prop, (uint32_t)DEVICE_SPECIFIC_PROP_VALUE);
      fuchsia_hardware_platform_bus::Metadata metadata = {
          {.data = std::vector<uint8_t>(reinterpret_cast<const uint8_t*>(&device_specific_prop),
                                        reinterpret_cast<const uint8_t*>(&device_specific_prop) +
                                            sizeof(device_specific_prop))}};
      node.AddMetadata(metadata);

      return zx::ok();
    }
    uint32_t device_specific_prop = 0;
  };

  DefaultVisitors<MetadataVisitor> visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());

  ASSERT_TRUE(DoPublish(manager).is_ok());

  ASSERT_EQ(1lu, publisher()->GetPbusNodes().size());
  ASSERT_EQ(10lu, publisher()->GetBoardChildNodes().size());

  // Check metadata of sample-device.
  auto metadata = publisher()->GetPbusNodes()[0].metadata();

  // Test Metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());
  ASSERT_EQ((uint32_t)DEVICE_SPECIFIC_PROP_VALUE,
            *reinterpret_cast<uint32_t*>((*(*metadata)[0].data()).data()));
}

TEST_F(ManagerTest, TestAddMetadataByPath) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  DefaultVisitors<> visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());

  uint32_t metadata_value = 42;
  fuchsia_hardware_platform_bus::Metadata metadata = {
      {.data = std::vector<uint8_t>(
           reinterpret_cast<const uint8_t*>(&metadata_value),
           reinterpret_cast<const uint8_t*>(&metadata_value) + sizeof(metadata_value))}};

  // Valid path.
  ASSERT_TRUE(manager.AddMetadata("/example-device", metadata).is_ok());

  // Non-existent path.
  ASSERT_TRUE(manager.AddMetadata("/non-existent-device", metadata).is_error());

  ASSERT_TRUE(DoPublish(manager).is_ok());

  // Check that the node has been published as a pbus node (since it now has metadata).
  ASSERT_EQ(1lu, publisher()->GetPbusNodes().size());
  auto pbus_node = publisher()->GetPbusNodes()[0];
  ASSERT_EQ(pbus_node.name(), "example-device");

  // Check metadata.
  ASSERT_TRUE(pbus_node.metadata());
  ASSERT_EQ(1lu, pbus_node.metadata()->size());
  ASSERT_EQ(metadata_value,
            *reinterpret_cast<uint32_t*>((*(*pbus_node.metadata())[0].data()).data()));
}

TEST_F(ManagerTest, TestReferences) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/basic-properties.dtb"));

  class ReferenceParentVisitor final : public DriverVisitor {
   public:
    using Property1Specifier = devicetree::PropEncodedArrayElement<PROPERTY1_CELLS>;

    ReferenceParentVisitor() : DriverVisitor({"fuchsia,reference-parent"}) {
      Properties props = {};
      props.emplace_back(std::make_unique<ReferenceProperty>("property1", "#property1-cells"));
      props.emplace_back(std::make_unique<ReferenceProperty>("property2", "#property2-cells"));
      props.emplace_back(
          std::make_unique<StringListProperty>("property2-names", /* required */ false));
      parser_ = std::make_unique<PropertyParser>(std::move(props));
    }

    zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      auto parser_output = parser_->Parse(node);
      if (parser_output.is_error()) {
        return parser_output.take_error();
      }

      if (auto property1 = parser_output->Get<References>("property1"); property1) {
        for (auto& reference : *property1) {
          if (is_match(reference.reference_node().properties())) {
            reference1_specifier() =
                devicetree::PropEncodedArray<Property1Specifier>(reference.property_cells(), 1);
            reference1_count()++;
          }
        }
      }

      auto property2 = parser_output->Get<References>("property2");
      auto property2_names = parser_output->Get<std::vector<std::string>>("property2-names");
      if (property2 && property2_names) {
        size_t index = 0;
        for (auto& reference : *property2) {
          if (is_match(reference.reference_node().properties())) {
            auto name = (*property2_names)[index];
            reference2_names().emplace_back(name);
            reference2_parent_names().push_back(reference.reference_node().name());
            reference2_count()++;
          }
          index++;
        }
      }

      return DriverVisitor::Visit(node, decoder);
    }

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      visit_called()++;
      return zx::ok();
    }

    zx::result<> DriverFinalizeNode(Node& node) override {
      ZX_ASSERT(reference1_count() == 1u);
      ZX_ASSERT(reference2_count() == 3u);
      finalize_called()++;
      return zx::ok();
    }

    size_t& visit_called() { return visit_called_; }
    size_t& finalize_called() { return finalize_called_; }
    size_t& reference1_count() { return reference1_count_; }
    size_t& reference2_count() { return reference2_count_; }
    devicetree::PropEncodedArray<Property1Specifier>& reference1_specifier() {
      return reference1_specifier_;
    }
    std::vector<std::string>& reference2_names() { return reference2_names_; }
    std::vector<std::string>& reference2_parent_names() { return reference2_parent_names_; }

   private:
    size_t visit_called_ = 0;
    size_t finalize_called_ = 0;
    size_t reference1_count_ = 0;
    size_t reference2_count_ = 0;
    devicetree::PropEncodedArray<Property1Specifier> reference1_specifier_;
    std::vector<std::string> reference2_names_;
    std::vector<std::string> reference2_parent_names_;
    std::unique_ptr<PropertyParser> parser_;
  };

  auto parent_visitor = std::make_unique<ReferenceParentVisitor>();
  ReferenceParentVisitor* parent_visitor_ptr = parent_visitor.get();

  VisitorRegistry visitors;
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(parent_visitor)).is_ok());

  ASSERT_EQ(ZX_OK, manager.Walk(visitors).status_value());

  ASSERT_EQ(parent_visitor_ptr->visit_called(), 3u);
  ASSERT_EQ(parent_visitor_ptr->finalize_called(), 3u);

  ASSERT_EQ(parent_visitor_ptr->reference1_specifier().size(), 1u);
  ASSERT_EQ(parent_visitor_ptr->reference1_specifier()[0][0], PROPERTY1_SPECIFIER);

  ASSERT_EQ(parent_visitor_ptr->reference2_parent_names()[0], "reference-parent-1");
  ASSERT_EQ(parent_visitor_ptr->reference2_parent_names()[1], "reference-parent-2");
  ASSERT_EQ(parent_visitor_ptr->reference2_parent_names()[2], "reference-parent-3");
  ASSERT_EQ(parent_visitor_ptr->reference2_names()[0], PROPERTY2_NAME1);
  ASSERT_EQ(parent_visitor_ptr->reference2_names()[1], PROPERTY2_NAME2);
  ASSERT_EQ(parent_visitor_ptr->reference2_names()[2], PROPERTY2_NAME3);

  ASSERT_TRUE(DoPublish(manager).is_ok());
}

TEST_F(ManagerTest, TestParentChild) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/basic-properties.dtb"));

  class ParentVisitor final : public DriverVisitor {
   public:
    ParentVisitor() : DriverVisitor({"fuchsia,parent"}) {}

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      auto children = node.children();
      child_count = children.size();
      for (ChildNode& child : children) {
        child_names.push_back(child.name());
      }
      name = node.name();
      return zx::ok();
    }
    size_t child_count = 0;
    std::vector<std::string_view> child_names;
    std::string_view name;
  };

  class ChildVisitor final : public DriverVisitor {
   public:
    ChildVisitor() : DriverVisitor({"fuchsia,child"}) {}

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      count++;
      if (!parent_name.empty() && parent_name != node.parent().name()) {
        return zx::error(ZX_ERR_INTERNAL);
      }
      parent_name = node.parent().name();
      names.push_back(node.name());
      return zx::ok();
    }
    size_t count = 0;
    std::vector<std::string_view> names;
    std::string_view parent_name;
  };

  auto parent_visitor = std::make_unique<ParentVisitor>();
  ParentVisitor* parent_visitor_ptr = parent_visitor.get();

  auto child_visitor = std::make_unique<ChildVisitor>();
  ChildVisitor* child_visitor_ptr = child_visitor.get();

  VisitorRegistry visitors;
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(parent_visitor)).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(child_visitor)).is_ok());

  ASSERT_EQ(ZX_OK, manager.Walk(visitors).status_value());

  EXPECT_EQ(parent_visitor_ptr->child_count, child_visitor_ptr->count);
  EXPECT_EQ(child_visitor_ptr->count, 2u);
  for (auto child_name : parent_visitor_ptr->child_names) {
    bool matched = false;
    for (auto name : child_visitor_ptr->names) {
      if (name == child_name) {
        matched = true;
        break;
      }
    }
    EXPECT_TRUE(matched);
  }
  EXPECT_EQ(child_visitor_ptr->parent_name, parent_visitor_ptr->name);

  ASSERT_TRUE(DoPublish(manager).is_ok());
}

TEST_F(ManagerTest, TestSkipDisabledNodes) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/status-disabled.dtb"));
  DefaultVisitors<> default_visitors;
  ASSERT_EQ(ZX_OK, manager.Walk(default_visitors).status_value());

  ASSERT_TRUE(DoPublish(manager).is_ok());
  ASSERT_EQ(0lu, publisher()->GetPbusNodes().size());
  ASSERT_EQ(3lu, publisher()->GetBoardChildNodes().size());

  auto board_child_node0 = publisher()->GetBoardChildNodes()[0];

  ASSERT_EQ(board_child_node0.name, "dt-root");
  auto board_child_node1 = publisher()->GetBoardChildNodes()[1];

  ASSERT_NE(nullptr, strstr("status-okay-device", board_child_node1.name.data()));
  auto board_child_node2 = publisher()->GetBoardChildNodes()[2];

  ASSERT_NE(nullptr, strstr("status-none-device", board_child_node2.name.data()));
}

TEST_F(ManagerTest, TestOverrideDisabledNodes) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/status-disabled.dtb"));
  manager.SetForceEnabledNodes({"/status-disabled-device"});
  DefaultVisitors<> default_visitors;
  ASSERT_EQ(ZX_OK, manager.Walk(default_visitors).status_value());

  ASSERT_TRUE(DoPublish(manager).is_ok());
  ASSERT_EQ(0lu, publisher()->GetPbusNodes().size());
  ASSERT_EQ(4lu, publisher()->GetBoardChildNodes().size());

  bool found_disabled = false;
  for (const BoardChildNode& node : publisher()->GetBoardChildNodes()) {
    if (node.name == "status-disabled-device") {
      found_disabled = true;
      break;
    }
  }
  EXPECT_TRUE(found_disabled);
}

TEST_F(ManagerTest, TestBoardChildCompositeSpec) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  static const std::string kTestKey = "test-key";
  static const std::string kTestProperty = "test-property";

  class TestDriverVisitor final : public DriverVisitor {
   public:
    TestDriverVisitor() : DriverVisitor({SAMPLE_DEVICE_COMPATIBILITY}) {}

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      visited = true;
      parent_spec.bind_rules({fdf::MakeAcceptBindRule(kTestKey, kTestProperty)});
      parent_spec.properties({fdf::MakeProperty2(kTestKey, kTestProperty)});
      node.AddNodeSpec(parent_spec);
      return zx::ok();
    }
    bool visited = false;
    fuchsia_driver_framework::ParentSpec2 parent_spec;
  };

  DefaultVisitors<TestDriverVisitor> visitor;

  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());
  ASSERT_TRUE(DoPublish(manager).is_ok());

  ASSERT_EQ(0lu, publisher()->GetPbusNodes().size());
  ASSERT_EQ(2lu, publisher()->GetBoardChildNodes().size());
  ASSERT_EQ(2lu, publisher()->GetCompositeNodeSpecs().size());

  auto mgr_request = publisher()->GetCompositeNodeSpecs()[1];
  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  EXPECT_TRUE(testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::COMPATIBLE, SAMPLE_DEVICE_COMPATIBILITY),
      }},
      (*mgr_request.parents2())[0].properties(), true));
  EXPECT_TRUE(testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::COMPATIBLE, SAMPLE_DEVICE_COMPATIBILITY),
      },
      (*mgr_request.parents2())[0].bind_rules(), true));

  EXPECT_TRUE(testing::CheckHasProperties({{fdf::MakeProperty2(kTestKey, kTestProperty)}},
                                          (*mgr_request.parents2())[1].properties(), false));
  EXPECT_TRUE(testing::CheckHasBindRules({{fdf::MakeAcceptBindRule(kTestKey, kTestProperty)}},
                                         (*mgr_request.parents2())[1].bind_rules(), false));
}

TEST_F(ManagerTest, TestPbusCompositeSpec) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  static const std::string kTestKey = "test-key";
  static const std::string kTestProperty = "test-property";

  class TestDriverVisitor final : public DriverVisitor {
   public:
    TestDriverVisitor() : DriverVisitor({SAMPLE_DEVICE_COMPATIBILITY}) {}

    zx::result<> DriverVisit(Node& node, const devicetree::PropertyDecoder& decoder) override {
      visited = true;
      parent_spec.bind_rules({fdf::MakeAcceptBindRule(kTestKey, kTestProperty)});
      parent_spec.properties({fdf::MakeProperty2(kTestKey, kTestProperty)});
      node.AddNodeSpec(parent_spec);
      // This adds a pbus resource, making the one of the parent of the composite to be platform
      // device.
      node.AddBootMetadata({});
      return zx::ok();
    }
    bool visited = false;
    fuchsia_driver_framework::ParentSpec2 parent_spec;
  };

  DefaultVisitors<TestDriverVisitor> visitor;

  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());
  ASSERT_TRUE(DoPublish(manager).is_ok());

  ASSERT_EQ(1lu, publisher()->GetPbusNodes().size());
  ASSERT_EQ(1lu, publisher()->GetBoardChildNodes().size());
  ASSERT_EQ(2lu, publisher()->GetCompositeNodeSpecs().size());

  auto mgr_request = publisher()->GetCompositeNodeSpecs()[1];
  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  EXPECT_TRUE(testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_platform::BIND_PROTOCOL_DEVICE),
      }},
      (*mgr_request.parents2())[0].properties(), true));
  EXPECT_TRUE(testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL,
                                  bind_fuchsia_platform::BIND_PROTOCOL_DEVICE),
      },
      (*mgr_request.parents2())[0].bind_rules(), true));

  EXPECT_TRUE(testing::CheckHasProperties({{fdf::MakeProperty2(kTestKey, kTestProperty)}},
                                          (*mgr_request.parents2())[1].properties(), false));
  EXPECT_TRUE(testing::CheckHasBindRules({{fdf::MakeAcceptBindRule(kTestKey, kTestProperty)}},
                                         (*mgr_request.parents2())[1].bind_rules(), false));
}

TEST_F(ManagerTest, TestPublishOrder) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/simple.dtb"));
  DefaultVisitors<> visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());
  auto& first_node = manager.nodes()[0];
  auto first_node_id = first_node->id();
  auto& second_node = manager.nodes()[1];
  auto second_node_id = second_node->id();
  EXPECT_EQ(first_node->GetPublishIndex(), 0u);
  EXPECT_EQ(second_node->GetPublishIndex(), 1u);
  EXPECT_TRUE(first_node->ChangePublishOrder(1u).is_ok());
  EXPECT_EQ(manager.nodes()[0]->id(), second_node_id);
  EXPECT_EQ(manager.nodes()[1]->id(), first_node_id);
  ASSERT_TRUE(DoPublish(manager).is_ok());
}

TEST_F(ManagerTest, GetPropertyTest) {
  Manager manager(testing::LoadTestBlob("/pkg/test-data/basic-properties.dtb"));
  DefaultVisitors<> visitor;
  ASSERT_EQ(ZX_OK, manager.Walk(visitor).status_value());

  auto test_device = manager.FindNode("test-properties-device");
  ASSERT_TRUE(test_device.has_value());

  // Test bool.
  ASSERT_TRUE((*test_device)->GetProperty<bool>("bool-property"));
  ASSERT_FALSE((*test_device)->GetProperty<bool>("non-existent-property"));

  // Test string.
  auto string_prop = (*test_device)->GetProperty<std::string>("string-property");
  ASSERT_TRUE(string_prop.is_ok());
  ASSERT_EQ(string_prop.value(), "hello");

  // Test uint32.
  auto uint32_prop = (*test_device)->GetProperty<uint32_t>("device_specific_prop");
  ASSERT_TRUE(uint32_prop.is_ok());
  ASSERT_EQ(uint32_prop.value(), static_cast<uint32_t>(DEVICE_SPECIFIC_PROP_VALUE));

  // Test uint64.
  auto uint64_prop = (*test_device)->GetProperty<uint64_t>("uint64-property");
  ASSERT_TRUE(uint64_prop.is_ok());
  ASSERT_EQ(uint64_prop.value(), 0x123456789abcdef0ULL);

  // Test uint32 vector.
  auto uint32_vector_prop =
      (*test_device)->GetProperty<std::vector<uint32_t>>("uint32-vector-property");
  ASSERT_TRUE(uint32_vector_prop.is_ok());
  ASSERT_EQ(uint32_vector_prop.value().size(), 2u);
  ASSERT_EQ(uint32_vector_prop.value()[0], 1u);
  ASSERT_EQ(uint32_vector_prop.value()[1], 2u);

  // Test string vector.
  auto string_vector_prop =
      (*test_device)->GetProperty<std::vector<std::string>>("string-list-property");
  ASSERT_TRUE(string_vector_prop.is_ok());
  ASSERT_EQ(string_vector_prop.value().size(), 2u);
  ASSERT_EQ(string_vector_prop.value()[0], "string1");
  ASSERT_EQ(string_vector_prop.value()[1], "string2");

  // Test wrong type.
  auto wrong_type_prop = (*test_device)->GetProperty<uint64_t>("string-property");
  ASSERT_TRUE(wrong_type_prop.is_error());
  ASSERT_EQ(wrong_type_prop.status_value(), ZX_ERR_WRONG_TYPE);

  // Test not found.
  auto not_found_prop = (*test_device)->GetProperty<std::string>("non-existent-property");
  ASSERT_TRUE(not_found_prop.is_error());
  ASSERT_EQ(not_found_prop.status_value(), ZX_ERR_NOT_FOUND);
}

}  // namespace
}  // namespace fdf_devicetree
