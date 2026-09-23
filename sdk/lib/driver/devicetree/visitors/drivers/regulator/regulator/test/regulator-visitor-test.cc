// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../regulator-visitor.h"

#include <fidl/fuchsia.hardware.vreg/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/regulator-test.h"
namespace regulator_visitor_dt {

class RegulatorVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<RegulatorVisitor> {
 public:
  RegulatorVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<RegulatorVisitor>(dtb_path,
                                                                     "RegulatorVisitorTest") {}
};

TEST(RegulatorVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<RegulatorVisitorTester>("/pkg/test-data/regulator.dtb");
  RegulatorVisitorTester* regulator_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, regulator_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(regulator_visitor_tester->DoPublish().is_ok());

  uint32_t node_tested_count = 0;

  std::vector<fuchsia_hardware_platform_bus::Node> pbus_nodes =
      regulator_visitor_tester->GetPbusNodes("voltage-regulator");
  ASSERT_EQ(1lu, pbus_nodes.size());
  auto& node = pbus_nodes[0];
  node_tested_count++;
  auto metadata = node.metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result vreg_metadata = fidl::Unpersist<fuchsia_hardware_vreg::VregMetadata>(metadata_blob);
  ASSERT_TRUE(vreg_metadata.is_ok());
  EXPECT_EQ(vreg_metadata->name(), REGULATOR_NAME);
  EXPECT_EQ(vreg_metadata->min_voltage_uv(), static_cast<uint32_t>(MIN_VOLTAGE));
  EXPECT_EQ(vreg_metadata->voltage_step_uv(), static_cast<uint32_t>(STEP_VOLTAGE));
  EXPECT_EQ(vreg_metadata->num_steps(),
            static_cast<uint32_t>((MAX_VOLTAGE - MIN_VOLTAGE) / STEP_VOLTAGE) + 1);

  auto child_nodes = regulator_visitor_tester->GetBoardChildNodes("cpu-ctrl");
  ASSERT_EQ(1lu, child_nodes.size());
  node_tested_count++;
  ASSERT_EQ(3lu, regulator_visitor_tester->GetCompositeNodeSpecs().size());

  auto mgr_requests = regulator_visitor_tester->GetCompositeNodeSpecs("cpu-ctrl");
  ASSERT_EQ(1lu, mgr_requests.size());
  auto mgr_request = mgr_requests[0];
  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  // Check for regulator parent node specs. Skip the 1st one as it is either pdev/board device.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, REGULATOR_FUNCTION),
      },
      (*mgr_request.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::NAME, REGULATOR_NAME)}},
      (*mgr_request.parents2())[1].bind_rules(), false));

  ASSERT_EQ(node_tested_count, 2u);
}

TEST(RegulatorVisitorTest, TestSharedRegulatorInstanceIds) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester =
      std::make_unique<RegulatorVisitorTester>("/pkg/test-data/multiclient-regulator.dtb");
  RegulatorVisitorTester* regulator_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, regulator_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(regulator_visitor_tester->DoPublish().is_ok());

  uint32_t node_tested_count = 0;

  for (const auto& node : regulator_visitor_tester->GetBoardChildNodes()) {
    if (node.name.find("cpu-ctrl") != std::string::npos ||
        node.name.find("gpu-ctrl") != std::string::npos) {
      node_tested_count++;
      ASSERT_EQ(4lu, regulator_visitor_tester->GetCompositeNodeSpecs().size());

      auto mgr_requests = regulator_visitor_tester->GetCompositeNodeSpecs(node.name);
      ASSERT_EQ(1lu, mgr_requests.size());
      auto mgr_request = mgr_requests[0];
      ASSERT_TRUE(mgr_request.parents2().has_value());
      ASSERT_EQ(2lu, mgr_request.parents2()->size());

      // Check for regulator parent node specs. Skip the 1st one as it is either pdev/board device.
      // When regulator-functions is omitted, FUNCTION property must not be generated.
      EXPECT_EQ(2lu, (*mgr_request.parents2())[1].properties().size());
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, REGULATOR_NAME),
          },
          (*mgr_request.parents2())[1].properties(), true));
      EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
          {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::NAME, REGULATOR_NAME)}},
          (*mgr_request.parents2())[1].bind_rules(), false));
    }
  }

  ASSERT_EQ(node_tested_count, 2u);
}

TEST(RegulatorVisitorTest, TestMismatchedRegulatorFunctions) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<RegulatorVisitorTester>("/pkg/test-data/invalid-regulator.dtb");
  RegulatorVisitorTester* regulator_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  EXPECT_EQ(ZX_ERR_INTERNAL, regulator_visitor_tester->manager()->Walk(visitors).status_value());
}

}  // namespace regulator_visitor_dt
