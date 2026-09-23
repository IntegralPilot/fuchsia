// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../pwm-init-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pwm/cpp/bind.h>
#include <gtest/gtest.h>

namespace pwm_init_visitor_dt {

class PwmInitVisitorTester final
    : public fdf_devicetree::testing::VisitorTestHelper<PwmInitVisitor> {
 public:
  explicit PwmInitVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<PwmInitVisitor>(dtb_path, "PwmInitVisitorTest") {
  }
};

TEST(PwmInitVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<PwmInitVisitorTester>("/pkg/test-data/pwm-init.dtb");
  PwmInitVisitorTester* pwm_init_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, pwm_init_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(pwm_init_visitor_tester->DoPublish().is_ok());

  auto specs = pwm_init_visitor_tester->GetCompositeNodeSpecs("wifi");
  ASSERT_EQ(1lu, specs.size());
  auto mgr_request = specs[0];
  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM)}},
      (*mgr_request.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM),
      }},
      (*mgr_request.parents2())[1].properties(), false));
}

}  // namespace pwm_init_visitor_dt
