// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/visitors/drivers/spi-controllers/spi-bus-visitor/spi-bus-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <optional>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

namespace spi_bus_dt {

class SpiBusVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<SpiBusVisitor> {
 public:
  explicit SpiBusVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<SpiBusVisitor>(dtb_path, "SpiBusVisitorTest") {}
};

TEST(SpiBusVisitorTest, TestSpiChannels) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpiBusVisitorTester* spi_tester = nullptr;
  {
    auto tester = std::make_unique<SpiBusVisitorTester>("/pkg/test-data/spi.dtb");
    spi_tester = tester.get();
    ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());
  }

  ASSERT_EQ(ZX_OK, spi_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(spi_tester->DoPublish().is_ok());

  auto pbus_nodes = spi_tester->GetPbusNodes("spi-");
  ASSERT_EQ(1lu, pbus_nodes.size());
  const auto& pbus_node = pbus_nodes[0];

  const std::optional<std::vector<fuchsia_hardware_platform_bus::Metadata>>& metadata =
      pbus_node.metadata();

  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  // SPI bus metadata
  {
    const std::vector<uint8_t>& metadata_blob = *(*metadata)[0].data();
    fit::result decoded =
        fidl::Unpersist<fuchsia_hardware_spi_businfo::SpiBusMetadata>(cpp20::span(metadata_blob));
    ASSERT_TRUE(decoded.is_ok());
    ASSERT_EQ(decoded->bus_id(), 0u);
    ASSERT_TRUE(decoded->channels());

    const std::vector<fuchsia_hardware_spi_businfo::SpiChannel>& channels = *decoded->channels();
    ASSERT_EQ(channels.size(), 4lu);
    EXPECT_EQ(channels[0].cs(), 0);
    ASSERT_TRUE(channels[0].max_frequency_hz().has_value());
    EXPECT_EQ(*channels[0].max_frequency_hz(), 1'000'000u);
    ASSERT_TRUE(channels[0].global_id().has_value());
    EXPECT_EQ(*channels[0].global_id(), 0u);

    EXPECT_EQ(channels[1].cs(), 1);
    EXPECT_FALSE(channels[1].max_frequency_hz().has_value());
    ASSERT_TRUE(channels[1].global_id().has_value());
    EXPECT_EQ(*channels[1].global_id(), 1u);

    EXPECT_EQ(channels[2].cs(), 2);
    ASSERT_TRUE(channels[2].max_frequency_hz().has_value());
    EXPECT_EQ(*channels[2].max_frequency_hz(), 5'000'000u);
    ASSERT_TRUE(channels[2].global_id().has_value());
    EXPECT_EQ(*channels[2].global_id(), 2u);

    EXPECT_EQ(channels[3].cs(), 3);
    ASSERT_TRUE(channels[3].max_frequency_hz().has_value());
    EXPECT_EQ(*channels[3].max_frequency_hz(), 5'000'000u);
    ASSERT_TRUE(channels[3].global_id().has_value());
    EXPECT_EQ(*channels[3].global_id(), 3u);
  }

  auto child0_specs = spi_tester->GetCompositeNodeSpecs("child-0");
  ASSERT_EQ(1lu, child0_specs.size());
  const auto& child0_spec = child0_specs[0];

  ASSERT_TRUE(child0_spec.parents2());
  ASSERT_EQ(child0_spec.parents2()->size(), 2ul);

  // The 0th parent is the board driver.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, "spi"),
      }},
      (*child0_spec.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u),
      }},
      (*child0_spec.parents2())[1].bind_rules(), false));

  auto child1_specs = spi_tester->GetCompositeNodeSpecs("child-1");
  ASSERT_EQ(1lu, child1_specs.size());
  const auto& child1_spec = child1_specs[0];

  ASSERT_TRUE(child1_spec.parents2());
  ASSERT_EQ(child1_spec.parents2()->size(), 2ul);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, "spi"),
      }},
      (*child1_spec.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 1u),
      }},
      (*child1_spec.parents2())[1].bind_rules(), false));

  auto child2_specs = spi_tester->GetCompositeNodeSpecs("child-2");
  ASSERT_EQ(1lu, child2_specs.size());
  const auto& child2_spec = child2_specs[0];

  ASSERT_TRUE(child2_spec.parents2());
  ASSERT_EQ(child2_spec.parents2()->size(), 3ul);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, "spi"),
      }},
      (*child2_spec.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 2u),
      }},
      (*child2_spec.parents2())[1].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, "spi"),
      }},
      (*child2_spec.parents2())[2].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 3u),
      }},
      (*child2_spec.parents2())[2].bind_rules(), false));
}

}  // namespace spi_bus_dt
