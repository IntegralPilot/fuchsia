// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../power-domain-visitor.h"

#include <fidl/fuchsia.hardware.powerdomain/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/power/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/power-domain-test.h"

namespace power_domain_visitor_dt {

class PowerDomainVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<PowerDomainVisitor> {
 public:
  PowerDomainVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<PowerDomainVisitor>(dtb_path,
                                                                       "PowerDomainVisitorTest") {}
};

TEST(PowerDomainVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<PowerDomainVisitorTester>("/pkg/test-data/power-domain.dtb");
  PowerDomainVisitorTester* power_domain_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, power_domain_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(power_domain_visitor_tester->DoPublish().is_ok());

  // Test the composite node spec
  auto cpufreq_node_spec = power_domain_visitor_tester->GetCompositeNodeSpecs("cpufreq");
  ASSERT_EQ(cpufreq_node_spec.size(), 1u);
  ASSERT_TRUE(cpufreq_node_spec[0].parents2().has_value());
  ASSERT_EQ(cpufreq_node_spec[0].parents2()->size(), 2lu);

  // 1st parent is pdev. Skipping that.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.power.Service"),
        fdf::MakeProperty2(bind_fuchsia_power::POWER_DOMAIN, static_cast<uint32_t>(TEST_DOMAIN_ID)),
        fdf::MakeProperty2(bind_fuchsia::NAME, "ice")}},
      cpufreq_node_spec[0].parents2()->at(1).properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.power.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN,
                                static_cast<uint32_t>(TEST_DOMAIN_ID))}},
      cpufreq_node_spec[0].parents2()->at(1).bind_rules(), false));

  // Test the power controller metadata
  auto power_controller_node = power_domain_visitor_tester->GetPbusNodes("power-controller");
  ASSERT_EQ(power_controller_node.size(), 1u);
  auto metadata = power_controller_node[0].metadata();
  ASSERT_TRUE(metadata);
  ASSERT_EQ(metadata->size(), 1u);
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result domain_metadata =
      fidl::Unpersist<fuchsia_hardware_power::DomainMetadata>(metadata_blob);
  ASSERT_TRUE(domain_metadata.is_ok());
  ASSERT_TRUE(domain_metadata->domains());
  ASSERT_EQ(domain_metadata->domains()->size(), 1u);
  ASSERT_EQ(domain_metadata->domains()->at(0).id(), static_cast<uint32_t>(TEST_DOMAIN_ID));
}

TEST(PowerDomainVisitorTest, TestBasicPowerDomain) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<PowerDomainVisitorTester>("/pkg/test-data/power-domain.dtb");
  PowerDomainVisitorTester* power_domain_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, power_domain_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(power_domain_visitor_tester->DoPublish().is_ok());

  // Test the composite node spec for basic device
  auto device_basic_node_spec =
      power_domain_visitor_tester->GetCompositeNodeSpecs("device-basic-a");
  ASSERT_EQ(device_basic_node_spec.size(), 1u);
  ASSERT_TRUE(device_basic_node_spec[0].parents2().has_value());
  ASSERT_EQ(device_basic_node_spec[0].parents2()->size(), 2lu);

  // 1st parent is pdev. Skipping that.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.powerdomain.Service"),
        fdf::MakeProperty2(bind_fuchsia_power::POWER_DOMAIN, 2u)}},
      device_basic_node_spec[0].parents2()->at(1).properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.powerdomain.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN, 2u),
        fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN_NODE_ID, 2u)}},
      device_basic_node_spec[0].parents2()->at(1).bind_rules(), false));

  // Test the composite node spec for basic device 2
  auto device_basic_2_node_spec =
      power_domain_visitor_tester->GetCompositeNodeSpecs("device-basic-b");
  ASSERT_EQ(device_basic_2_node_spec.size(), 1u);
  ASSERT_TRUE(device_basic_2_node_spec[0].parents2().has_value());
  ASSERT_EQ(device_basic_2_node_spec[0].parents2()->size(), 2lu);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.powerdomain.Service"),
        fdf::MakeProperty2(bind_fuchsia_power::POWER_DOMAIN, 2u),
        fdf::MakeProperty2(bind_fuchsia::NAME, "basic_power_2")}},
      device_basic_2_node_spec[0].parents2()->at(1).properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.powerdomain.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN, 2u),
        fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN_NODE_ID, 3u)}},
      device_basic_2_node_spec[0].parents2()->at(1).bind_rules(), false));

  // Test the basic power controller metadata
  auto power_controller_node = power_domain_visitor_tester->GetPbusNodes("basic-pwrc");
  ASSERT_EQ(power_controller_node.size(), 1u);
  auto metadata = power_controller_node[0].metadata();
  ASSERT_TRUE(metadata);
  ASSERT_EQ(metadata->size(), 1u);
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result domain_metadata =
      fidl::Unpersist<fuchsia_hardware_powerdomain::DomainMetadata>(metadata_blob);
  ASSERT_TRUE(domain_metadata.is_ok());
  ASSERT_TRUE(domain_metadata->domains());
  ASSERT_EQ(domain_metadata->domains()->size(), 2u);
  ASSERT_EQ(domain_metadata->domains()->at(0).id(), 2u);
  ASSERT_EQ(domain_metadata->domains()->at(0).node_id(), 2u);
  ASSERT_EQ(domain_metadata->domains()->at(1).id(), 2u);
  ASSERT_EQ(domain_metadata->domains()->at(1).node_id(), 3u);
  ASSERT_EQ(domain_metadata->domains()->at(1).name(), "basic_power_2");
}

}  // namespace power_domain_visitor_dt
