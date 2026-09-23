// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../ethernet-phy-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>
namespace eth_phy_visitor_dt {

class EthPhyVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<EthPhyVisitor> {
 public:
  EthPhyVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<EthPhyVisitor>(dtb_path, "EthPhyVisitorTest") {}
};

TEST(EthVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<EthPhyVisitorTester>("/pkg/test-data/ethernet-phy.dtb");
  EthPhyVisitorTester* eth_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, eth_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(eth_visitor_tester->DoPublish().is_ok());

  std::vector<fuchsia_hardware_platform_bus::Node> phy_nodes =
      eth_visitor_tester->GetPbusNodes("dwmac");
  ASSERT_EQ(1lu, phy_nodes.size());

  auto mgr_request = eth_visitor_tester->GetCompositeNodeSpecs("dwmac")[0];

  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  // 1st parent is pdev. Skip that.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.ethernet.board.Service")}},
      (*mgr_request.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.ethernet.board.Service"),
      }},
      (*mgr_request.parents2())[1].properties(), false));
}

}  // namespace eth_phy_visitor_dt
