// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../reset-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/reset/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/resets.h"

namespace reset_dt {

class ResetVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<ResetVisitor> {
 public:
  ResetVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<ResetVisitor>(dtb_path, "ResetVisitorTest") {}
};

TEST(ResetVisitorTest, TestResetProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  auto tester = std::make_unique<ResetVisitorTester>("/pkg/test-data/resets.dtb");
  ResetVisitorTester* reset_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, reset_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(reset_tester->DoPublish().is_ok());

  auto mgr_requests = reset_tester->GetCompositeNodeSpecs("test-device");
  ASSERT_EQ(1lu, mgr_requests.size());
  auto mgr_request = mgr_requests[0];
  const auto& parents = mgr_request.parents2();
  ASSERT_EQ(parents->size(), 4u);

  std::optional<unsigned int> reset_controller1_id;
  std::optional<unsigned int> reset_controller2_id;

  std::vector<fuchsia_hardware_platform_bus::Node> pbus_nodes =
      reset_tester->GetPbusNodes("reset-controller");
  for (const auto& node : pbus_nodes) {
    std::optional<unsigned int>* active_controller_id = nullptr;
    if (node.name()->find("first-reset-controller") != std::string::npos) {
      active_controller_id = &reset_controller1_id;
    } else if (node.name()->find("second-reset-controller") != std::string::npos) {
      active_controller_id = &reset_controller2_id;
    } else {
      continue;
    }
    auto metadata = node.metadata();

    // Test metadata properties.
    ASSERT_TRUE(metadata);
    ASSERT_EQ(1lu, metadata->size());
    std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
    fit::result decoded =
        fidl::Unpersist<fuchsia_hardware_reset::Metadata>(cpp20::span(metadata_blob));
    ASSERT_TRUE(decoded.is_ok());
    ASSERT_TRUE((*decoded).controller_id());
    *active_controller_id = decoded->controller_id();
  }

  // Make sure we found and parsed both controllers.
  ASSERT_TRUE(reset_controller1_id);
  ASSERT_TRUE(reset_controller2_id);

  // Parent 0 - pdev, ignore
  // Parent 1 - Reset with only one reset-cell, RESET_ID defaults to 0
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_reset::CONTROLLER_ID, reset_controller1_id.value()),
          fdf::MakeAcceptBindRule(bind_fuchsia_reset::RESET_ID, static_cast<unsigned int>(0)),
      }},
      (*parents)[1].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, RESET_NAME_0),
      }},
      (*parents)[1].properties(), false));

  // Parent 2 - Reset with multiple reset-cells
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_reset::CONTROLLER_ID, reset_controller2_id.value()),
          fdf::MakeAcceptBindRule(bind_fuchsia_reset::RESET_ID,
                                  static_cast<unsigned int>(RESET_ID_1)),
      }},
      (*parents)[2].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, RESET_NAME_1),
      }},
      (*parents)[2].properties(), false));

  // Parent 3 - Reset with multiple reset-cells
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_reset::CONTROLLER_ID, reset_controller2_id.value()),
          fdf::MakeAcceptBindRule(bind_fuchsia_reset::RESET_ID,
                                  static_cast<unsigned int>(RESET_ID_2)),
      }},
      (*parents)[3].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, RESET_NAME_2),
      }},
      (*parents)[3].properties(), false));
}

TEST(ResetVisitorTest, TestResetPropertyNoNames) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  auto tester = std::make_unique<ResetVisitorTester>("/pkg/test-data/resets-no-names.dtb");
  ResetVisitorTester* reset_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_NE(ZX_OK, reset_tester->manager()->Walk(visitors).status_value());
}

TEST(ResetVisitorTest, TestResetPropertyNamesMismatch) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  auto tester = std::make_unique<ResetVisitorTester>("/pkg/test-data/resets-name-mismatch.dtb");
  ResetVisitorTester* reset_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_NE(ZX_OK, reset_tester->manager()->Walk(visitors).status_value());
}

}  // namespace reset_dt
