// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../pwm-visitor.h"

#include <fidl/fuchsia.hardware.pwm/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/pwm.h"

namespace pwm_visitor_dt {

class PwmVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<PwmVisitor> {
 public:
  PwmVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<PwmVisitor>(dtb_path, "PwmVisitorTest") {}
};

TEST(PwmVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<PwmVisitorTester>("/pkg/test-data/pwm.dtb");
  PwmVisitorTester* pwm_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, pwm_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(pwm_visitor_tester->DoPublish().is_ok());

  // Check "pwm-ffffa000"
  {
    auto nodes = pwm_visitor_tester->GetPbusNodes("pwm-ffffa000");
    ASSERT_EQ(1lu, nodes.size());
    auto& node = nodes[0];
    auto metadata = node.metadata();

    // Test metadata properties.
    ASSERT_TRUE(metadata);
    ASSERT_EQ(1lu, metadata->size());

    // PWM Channels metadata.
    std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
    fit::result pwm_channels =
        fidl::Unpersist<fuchsia_hardware_pwm::PwmChannelsMetadata>(cpp20::span(metadata_blob));
    ASSERT_TRUE(pwm_channels.is_ok());

    ASSERT_TRUE(pwm_channels->channels());
    ASSERT_EQ(pwm_channels->channels()->size(), 2u);
    EXPECT_EQ((*pwm_channels->channels())[0].id(), static_cast<uint32_t>(PIN1));
    EXPECT_EQ((*pwm_channels->channels())[0].period_ns(), static_cast<uint32_t>(PIN1_PERIOD));
    EXPECT_EQ((*pwm_channels->channels())[0].polarity().value(), true);
    EXPECT_FALSE((*pwm_channels->channels())[0].skip_init());
    EXPECT_EQ((*pwm_channels->channels())[1].id(), static_cast<uint32_t>(PIN2));
    EXPECT_EQ((*pwm_channels->channels())[1].period_ns(), static_cast<uint32_t>(PIN2_PERIOD));
    EXPECT_EQ((*pwm_channels->channels())[1].polarity().value(), true);
    EXPECT_EQ((*pwm_channels->channels())[1].skip_init().value(), true);
  }

  // Check "pwm-ffffb000"
  {
    auto nodes = pwm_visitor_tester->GetPbusNodes("pwm-ffffb000");
    ASSERT_EQ(1lu, nodes.size());
    auto& node = nodes[0];
    auto metadata = node.metadata();

    // Test that there are no metadata properties as this pwm is not referenced by any nodes.
    ASSERT_FALSE(metadata);
  }

  // Check "audio"
  {
    auto specs = pwm_visitor_tester->GetCompositeNodeSpecs("audio");
    ASSERT_EQ(1lu, specs.size());
    auto mgr_request = specs[0];
    ASSERT_TRUE(mgr_request.parents2().has_value());
    ASSERT_EQ(3lu, mgr_request.parents2()->size());

    // 1st parent is pdev. Skip that.
    // Bind rules for PIN1
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(PIN1)),
        }},
        (*mgr_request.parents2())[1].bind_rules(), false));
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
        {{
            fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
            fdf::MakeProperty2(bind_fuchsia::NAME, std::string(PIN1_NAME)),
        }},
        (*mgr_request.parents2())[1].properties(), false));

    // Bind rules for PIN2
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(PIN2)),
        }},
        (*mgr_request.parents2())[2].bind_rules(), false));
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
        {{
            fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
            fdf::MakeProperty2(bind_fuchsia::NAME, std::string(PIN2_NAME)),
        }},
        (*mgr_request.parents2())[2].properties(), false));
  }
}

}  // namespace pwm_visitor_dt
