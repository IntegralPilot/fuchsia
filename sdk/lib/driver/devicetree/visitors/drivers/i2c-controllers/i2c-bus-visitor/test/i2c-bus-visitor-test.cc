// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../i2c-bus-visitor.h"

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/i2c.h"

namespace i2c_bus_dt {

class I2cBusVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<I2cBusVisitor> {
 public:
  explicit I2cBusVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<I2cBusVisitor>(dtb_path, "I2cBusVisitorTest") {}
};

TEST(I2cBusVisitorTest, TestI2CChannels) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<I2cBusVisitorTester>("/pkg/test-data/i2c.dtb");
  I2cBusVisitorTester* i2c_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, i2c_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(i2c_tester->DoPublish().is_ok());

  ASSERT_EQ(7lu, i2c_tester->GetCompositeNodeSpecs().size());

  uint32_t node_tested_count = 0;
  std::vector<fuchsia_hardware_platform_bus::Node> nodes = i2c_tester->GetPbusNodes("i2c-");
  for (const auto& node : nodes) {
    auto metadata = node.metadata();

    // Test metadata properties.
    ASSERT_TRUE(metadata);
    ASSERT_EQ(1lu, metadata->size());

    // I2C Channels metadata
    std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
    fit::result decoded =
        fidl::Unpersist<fuchsia_hardware_i2c_businfo::I2CBusMetadata>(cpp20::span(metadata_blob));
    ASSERT_TRUE(decoded.is_ok());
    ASSERT_EQ(decoded->bus_id(), 0u);
    auto& channels = *decoded->channels();
    ASSERT_EQ(channels.size(), 6lu);
    EXPECT_EQ(channels[0].address(), static_cast<uint32_t>(I2C_ADDRESS1));
    EXPECT_EQ(channels[0].global_id(), 0u);
    EXPECT_EQ(channels[1].address(), static_cast<uint32_t>(I2C_ADDRESS2));
    EXPECT_EQ(channels[1].global_id(), 1u);
    EXPECT_EQ(channels[2].address(), static_cast<uint32_t>(I2C_ADDRESS3));
    EXPECT_EQ(channels[2].global_id(), 2u);
    EXPECT_EQ(channels[3].address(), static_cast<uint32_t>(I2C_ADDRESS4));
    EXPECT_EQ(channels[3].global_id(), 3u);
    EXPECT_EQ(channels[4].address(), static_cast<uint32_t>(I2C_ADDRESS5));
    EXPECT_EQ(channels[4].global_id(), 4u);
    EXPECT_EQ(channels[5].address(), static_cast<uint32_t>(I2C_ADDRESS6));
    EXPECT_EQ(channels[5].global_id(), 5u);

    node_tested_count++;
  }

  for (auto& node : i2c_tester->GetBoardChildNodes("child-")) {
    std::string node_name = node.name;

    auto composite_node_specs = i2c_tester->GetCompositeNodeSpecs(node_name);
    ASSERT_EQ(1lu, composite_node_specs.size());
    fuchsia_driver_framework::CompositeNodeSpec composite_node_spec = composite_node_specs[0];
    ASSERT_TRUE(composite_node_spec.parents2().has_value());
    const std::vector<fuchsia_driver_framework::ParentSpec2>& parent_specs =
        *composite_node_spec.parents2();

    // The first parent is the pdev node and the rest parents are I2c nodes.
    ASSERT_GT(parent_specs.size(), 1lu);
    cpp20::span<const fuchsia_driver_framework::ParentSpec2> i2c_nodes(++parent_specs.begin(),
                                                                       parent_specs.end());

    if (node_name == "child-c") {
      ASSERT_EQ(i2c_nodes.size(), 1lu);
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
          },
          i2c_nodes[0].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u),
          }},
          i2c_nodes[0].bind_rules(), false));
    }

    if (node_name == "child-dup-c") {
      ASSERT_EQ(i2c_nodes.size(), 1lu);
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
          },
          i2c_nodes[0].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u),
          }},
          i2c_nodes[0].bind_rules(), false));
    }

    if (node_name == "child-1e") {
      ASSERT_EQ(i2c_nodes.size(), 1lu);
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
          },
          i2c_nodes[0].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 1u),
          }},
          i2c_nodes[0].bind_rules(), false));
    }

    if (node_name == "child-2b") {
      ASSERT_EQ(i2c_nodes.size(), 2lu);

      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
          },
          i2c_nodes[0].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 2u),
          }},
          i2c_nodes[0].bind_rules(), false));

      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
          },
          i2c_nodes[1].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 3u),
          }},
          i2c_nodes[1].bind_rules(), false));
    }

    if (node_name == "child-names-30") {
      ASSERT_EQ(i2c_nodes.size(), 2lu);

      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c-control"),
          },
          i2c_nodes[0].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 4u),
          }},
          i2c_nodes[0].bind_rules(), false));

      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c-data"),
          },
          i2c_nodes[1].properties(), false));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, 5u),
          }},
          i2c_nodes[1].bind_rules(), false));
    }

    node_tested_count++;
  }

  ASSERT_EQ(node_tested_count, 6u);
}

}  // namespace i2c_bus_dt
