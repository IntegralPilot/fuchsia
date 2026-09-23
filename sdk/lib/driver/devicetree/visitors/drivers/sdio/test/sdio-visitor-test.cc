// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../sdio-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/sdio/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/sdio.h"

namespace sdio_dt {

class SdioVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<SdioVisitor> {
 public:
  explicit SdioVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<SdioVisitor>(dtb_path, "SdioVisitorTest") {}
};

TEST(SdioVisitorTest, TestSdioFunctionDevices) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<SdioVisitorTester>("/pkg/test-data/sdio.dtb");
  SdioVisitorTester* sdio_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, sdio_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(sdio_tester->DoPublish().is_ok());

  // child-0 (func 0) is skipped so it should only have 1 parent spec (the default pdev parent
  // spec).
  auto child_0_specs = sdio_tester->GetCompositeNodeSpecs("child-0");
  ASSERT_EQ(1lu, child_0_specs.size());
  ASSERT_TRUE(child_0_specs[0].parents2().has_value());
  EXPECT_EQ(1lu, child_0_specs[0].parents2()->size());

  auto child_1_specs = sdio_tester->GetCompositeNodeSpecs("child-1");
  ASSERT_EQ(1lu, child_1_specs.size());

  fuchsia_driver_framework::CompositeNodeSpec composite_node_spec = child_1_specs[0];
  ASSERT_TRUE(composite_node_spec.parents2().has_value());
  const std::vector<fuchsia_driver_framework::ParentSpec2>& parent_specs =
      *composite_node_spec.parents2();

  // The first parent is the pdev node and the rest are SDIO nodes.
  ASSERT_EQ(3lu, parent_specs.size());
  cpp20::span<const fuchsia_driver_framework::ParentSpec2> sdio_nodes(parent_specs.begin() + 1,
                                                                      parent_specs.end());

  ASSERT_EQ(2lu, sdio_nodes.size());

  // Check parent spec for SDIO function 1.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.sdio.Service"),
          fdf::MakeProperty2(bind_fuchsia::SDIO_FUNCTION, uint32_t{SDIO_FUNCTION_1}),
      },
      sdio_nodes[0].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE),
          fdf::MakeAcceptBindRule(bind_fuchsia::SDIO_FUNCTION, uint32_t{SDIO_FUNCTION_1}),
      }},
      sdio_nodes[0].bind_rules(), false));

  // Check parent spec for SDIO function 2.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.sdio.Service"),
          fdf::MakeProperty2(bind_fuchsia::SDIO_FUNCTION, uint32_t{SDIO_FUNCTION_2}),
      },
      sdio_nodes[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE),
          fdf::MakeAcceptBindRule(bind_fuchsia::SDIO_FUNCTION, uint32_t{SDIO_FUNCTION_2}),
      }},
      sdio_nodes[1].bind_rules(), false));
}

}  // namespace sdio_dt
