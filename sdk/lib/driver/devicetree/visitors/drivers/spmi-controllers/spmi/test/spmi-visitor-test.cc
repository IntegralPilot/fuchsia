// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../spmi-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <memory>
#include <optional>
#include <string_view>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/spmi/cpp/bind.h>
#include <gtest/gtest.h>

namespace {

std::optional<fuchsia_hardware_spmi::TargetInfo> FindTargetById(
    uint8_t id, const fuchsia_hardware_spmi::ControllerInfo& controller) {
  if (!controller.targets()) {
    return {};
  }

  for (const auto& target : *controller.targets()) {
    if (target.id() && *target.id() == id) {
      return target;
    }
  }

  return {};
}

std::optional<fuchsia_hardware_spmi::SubTargetInfo> FindSubTargetByAddress(
    uint16_t address, const fuchsia_hardware_spmi::TargetInfo& target) {
  if (!target.sub_targets()) {
    return {};
  }

  for (const auto& sub_target : *target.sub_targets()) {
    if (sub_target.address() && *sub_target.address() == address) {
      return sub_target;
    }
  }

  return {};
}

}  // namespace

namespace spmi_dt {

class SpmiVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<SpmiVisitor> {
 public:
  explicit SpmiVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<SpmiVisitor>(dtb_path, "SpmiBusVisitorTest") {}
};

// Verifies that multiple SPMI controllers with targets and sub-targets are parsed correctly.
TEST(SpmiVisitorTest, TwoControllers) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester = new SpmiVisitorTester("/pkg/test-data/spmi.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  ASSERT_TRUE(spmi_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(spmi_tester->DoPublish().is_ok());

  // First controller metadata
  auto pbus_node_0_list = spmi_tester->GetPbusNodes("spmi-abcd0000");
  ASSERT_EQ(1u, pbus_node_0_list.size());
  const auto& pbus_node_0 = pbus_node_0_list[0];

  ASSERT_TRUE(pbus_node_0.metadata());
  ASSERT_EQ(pbus_node_0.metadata()->size(), 1u);

  ASSERT_TRUE((*pbus_node_0.metadata())[0].id());
  EXPECT_EQ(*(*pbus_node_0.metadata())[0].id(),
            fuchsia_hardware_spmi::ControllerInfo::kSerializableName);

  ASSERT_TRUE((*pbus_node_0.metadata())[0].data());
  const std::vector<uint8_t>& metadata_0 = *(*pbus_node_0.metadata())[0].data();

  const auto controller_0 = fidl::Unpersist<fuchsia_hardware_spmi::ControllerInfo>(
      {metadata_0.data(), metadata_0.size()});
  ASSERT_TRUE(controller_0.is_ok());

  ASSERT_TRUE(controller_0->id());
  const uint32_t controller_0_id = *controller_0->id();

  ASSERT_TRUE(controller_0->targets());
  ASSERT_EQ(controller_0->targets()->size(), 2u);

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_0 =
      FindTargetById(0, *controller_0);
  ASSERT_TRUE(target_0);

  ASSERT_TRUE(target_0->display_name());
  EXPECT_EQ(*target_0->display_name(), "target-a-0");

  ASSERT_TRUE(target_0->sub_targets());
  ASSERT_EQ(target_0->sub_targets()->size(), 4u);

  const std::optional<fuchsia_hardware_spmi::SubTargetInfo> sub_target_1000 =
      FindSubTargetByAddress(0x1000, *target_0);
  ASSERT_TRUE(sub_target_1000);

  ASSERT_TRUE(sub_target_1000->size());
  EXPECT_EQ(sub_target_1000->size(), 0x1000);

  EXPECT_FALSE(sub_target_1000->name());

  ASSERT_TRUE(sub_target_1000->display_name());
  EXPECT_EQ(*sub_target_1000->display_name(), "vreg-1000");

  const std::optional<fuchsia_hardware_spmi::SubTargetInfo> sub_target_2000 =
      FindSubTargetByAddress(0x2000, *target_0);
  ASSERT_TRUE(sub_target_2000);

  ASSERT_TRUE(sub_target_2000->size());
  EXPECT_EQ(sub_target_2000->size(), 0x800);

  EXPECT_FALSE(sub_target_2000->name());

  ASSERT_TRUE(sub_target_2000->display_name());
  EXPECT_EQ(*sub_target_2000->display_name(), "gpio-2000");

  const std::optional<fuchsia_hardware_spmi::SubTargetInfo> sub_target_3000 =
      FindSubTargetByAddress(0x3000, *target_0);
  ASSERT_TRUE(sub_target_3000);

  ASSERT_TRUE(sub_target_3000->size());
  EXPECT_EQ(sub_target_3000->size(), 0x400);

  ASSERT_TRUE(sub_target_3000->name());
  EXPECT_EQ(*sub_target_3000->name(), "i2c-core");

  ASSERT_TRUE(sub_target_3000->display_name());
  EXPECT_EQ(*sub_target_3000->display_name(), "i2c-3000");

  const std::optional<fuchsia_hardware_spmi::SubTargetInfo> sub_target_ffff =
      FindSubTargetByAddress(0xffff, *target_0);
  ASSERT_TRUE(sub_target_ffff);

  ASSERT_TRUE(sub_target_ffff->size());
  ASSERT_EQ(sub_target_ffff->size(), 1);

  ASSERT_TRUE(sub_target_ffff->name());
  EXPECT_EQ(*sub_target_ffff->name(), "i2c-config");

  ASSERT_TRUE(sub_target_ffff->display_name());
  EXPECT_EQ(*sub_target_ffff->display_name(), "i2c-3000");

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_3 =
      FindTargetById(3, *controller_0);
  ASSERT_TRUE(target_3);

  EXPECT_FALSE(target_3->sub_targets());

  ASSERT_TRUE(target_3->name());
  EXPECT_EQ(*target_3->name(), "vreg");

  ASSERT_TRUE(target_3->display_name());
  EXPECT_EQ(*target_3->display_name(), "target-b-3");

  // First controller composite node specs
  auto vreg_1000_list = spmi_tester->GetCompositeNodeSpecs("vreg-1000");
  ASSERT_EQ(1u, vreg_1000_list.size());
  const auto& vreg_1000 = vreg_1000_list[0];

  ASSERT_TRUE(vreg_1000.parents2());
  ASSERT_EQ(vreg_1000.parents2()->size(), 2u);

  // The 0th composite parent has the `compatible` string and is added by the default visitor. Start
  // at index 1 to skip this parent and validate only the parents2 added by the SPMI visitor.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_0_id),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x1000u),
      },
      (*vreg_1000.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeProperty2(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x1000u),
      },
      (*vreg_1000.parents2())[1].properties(), false));

  // gpio@2000 and i2c@3000 are referenced by another node, so no node specs should be
  // added for them.
  const auto gpio_2000_list = spmi_tester->GetCompositeNodeSpecs("gpio-2000");
  EXPECT_TRUE(gpio_2000_list.empty());

  const auto i2c_3000_list = spmi_tester->GetCompositeNodeSpecs("i2c-3000");
  EXPECT_TRUE(i2c_3000_list.empty());

  // Second controller metadata
  const auto pbus_node_1_list = spmi_tester->GetPbusNodes("spmi-abcf0000");
  ASSERT_EQ(1u, pbus_node_1_list.size());
  const auto& pbus_node_1 = pbus_node_1_list[0];

  ASSERT_TRUE(pbus_node_1.metadata());
  ASSERT_EQ(pbus_node_1.metadata()->size(), 1u);

  ASSERT_TRUE((*pbus_node_1.metadata())[0].id());
  EXPECT_EQ(*(*pbus_node_1.metadata())[0].id(),
            fuchsia_hardware_spmi::ControllerInfo::kSerializableName);

  ASSERT_TRUE((*pbus_node_1.metadata())[0].data());
  const std::vector<uint8_t>& metadata_1 = *(*pbus_node_1.metadata())[0].data();

  const auto controller_1 = fidl::Unpersist<fuchsia_hardware_spmi::ControllerInfo>(
      {metadata_1.data(), metadata_1.size()});
  ASSERT_TRUE(controller_1.is_ok());

  ASSERT_TRUE(controller_1->id());
  const uint32_t controller_1_id = *controller_1->id();

  ASSERT_TRUE(controller_1->targets());
  ASSERT_EQ(controller_1->targets()->size(), 1u);

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_1_0 =
      FindTargetById(0, *controller_1);
  ASSERT_TRUE(target_1_0);
  EXPECT_FALSE(target_1_0->sub_targets());

  // Second controller composite node specs
  const auto target_c_0_list = spmi_tester->GetCompositeNodeSpecs("target-c-0");
  ASSERT_EQ(1u, target_c_0_list.size());
  const auto& target_c_0 = target_c_0_list[0];

  ASSERT_TRUE(target_c_0.parents2());
  ASSERT_EQ(target_c_0.parents2()->size(), 2u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_1_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u),
      },
      (*target_c_0.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0u),
      },
      (*target_c_0.parents2())[1].properties(), false));

  // The second pbus node is not an SPMI controller and should not have metadata. It does have an
  // "spmis" property and should have parents2 for the SPMI sub-targets that it
  // references.

  const auto pbus_node_ignored_list = spmi_tester->GetPbusNodes("not-spmi-abce0000");
  ASSERT_EQ(1u, pbus_node_ignored_list.size());
  const auto& pbus_node_ignored = pbus_node_ignored_list[0];
  EXPECT_FALSE(pbus_node_ignored.metadata());

  const auto not_spmi_list = spmi_tester->GetCompositeNodeSpecs("not-spmi-abce0000");
  ASSERT_EQ(1u, not_spmi_list.size());
  const auto& not_spmi = not_spmi_list[0];

  ASSERT_TRUE(not_spmi.parents2());
  ASSERT_EQ(not_spmi.parents2()->size(), 4u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_0_id),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x2000u),
      },
      (*not_spmi.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeProperty2(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x2000u),
      },
      (*not_spmi.parents2())[1].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_0_id),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x3000u),
      },
      (*not_spmi.parents2())[2].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeProperty2(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x3000u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "i2c-core"),
      },
      (*not_spmi.parents2())[2].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_0_id),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0xffffu),
      },
      (*not_spmi.parents2())[3].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
          fdf::MakeProperty2(bind_fuchsia_spmi::TARGET_ID, 0u),
          fdf::MakeProperty2(bind_fuchsia::ID, 0xffffu),
          fdf::MakeProperty2(bind_fuchsia::NAME, "i2c-config"),
      },
      (*not_spmi.parents2())[3].properties(), false));
}

// Verifies that register type is correctly set to SPMI for targets and sub-targets.
TEST(SpmiVisitorTest, RegisterType) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester = new SpmiVisitorTester("/pkg/test-data/spmi.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  ASSERT_TRUE(spmi_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(spmi_tester->DoPublish().is_ok());

  std::vector<std::string> mmio_nodes = {"spmi@abcd0000", "spmi@abcf0000", "not-spmi@abce0000"};

  for (auto& mmio_node : mmio_nodes) {
    auto nodes = spmi_tester->GetDevicetreeNodes(mmio_node);
    ASSERT_EQ(1u, nodes.size());
    ASSERT_EQ(nodes[0]->register_type(), fdf_devicetree::RegisterType::kMmio);
  }

  std::vector<std::string> spmi_register_nodes = {"target-a@0", "vreg@1000",  "gpio@2000",
                                                  "i2c@3000",   "target-b@3", "target-c@0"};

  for (auto& spmi_register_node : spmi_register_nodes) {
    auto nodes = spmi_tester->GetDevicetreeNodes(spmi_register_node);
    ASSERT_EQ(1u, nodes.size());
    ASSERT_EQ(nodes[0]->register_type(), fdf_devicetree::RegisterType::kSpmi);
  }
}

// Verifies that sub-target SPMI addresses out of range are rejected.
TEST(SpmiVisitorTest, SubTargetSpmiAddressOutOfRange) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-sub-target-spmi-address-out-of-range.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  EXPECT_FALSE(spmi_tester->manager()->Walk(visitors).is_ok());
}

// Verifies that a devicetree node can reference an SPMI target and attach its TargetService spec.
TEST(SpmiVisitorTest, PropertyReferencesTarget) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-reference-target.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  ASSERT_TRUE(spmi_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(spmi_tester->DoPublish().is_ok());

  auto pbus_node_list = spmi_tester->GetPbusNodes("spmi-abcd0000");
  ASSERT_EQ(1u, pbus_node_list.size());
  const auto& pbus_node = pbus_node_list[0];

  ASSERT_TRUE(pbus_node.metadata());
  ASSERT_EQ(pbus_node.metadata()->size(), 1u);

  const std::vector<uint8_t>& metadata = *(*pbus_node.metadata())[0].data();
  const auto controller =
      fidl::Unpersist<fuchsia_hardware_spmi::ControllerInfo>({metadata.data(), metadata.size()});
  ASSERT_TRUE(controller.is_ok());
  ASSERT_TRUE(controller->id());
  const uint32_t controller_id = *controller->id();

  // target-a@0 is referenced by not-spmi@abce0000, so no node specs should be added for it.
  const auto target_a_list = spmi_tester->GetCompositeNodeSpecs("target-a-0");
  EXPECT_TRUE(target_a_list.empty());

  const auto not_spmi_list = spmi_tester->GetCompositeNodeSpecs("not-spmi-abce0000");
  ASSERT_EQ(1u, not_spmi_list.size());
  const auto& not_spmi = not_spmi_list[0];

  ASSERT_TRUE(not_spmi.parents2());
  ASSERT_EQ(not_spmi.parents2()->size(), 2u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u),
      },
      (*not_spmi.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "target-a"),
      },
      (*not_spmi.parents2())[1].properties(), false));
}

// Verifies that multiple devicetree nodes referencing the same SPMI target is rejected.
TEST(SpmiVisitorTest, TwoNodesReferenceTarget) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-two-nodes-reference-target.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  EXPECT_FALSE(spmi_tester->manager()->Walk(visitors).is_ok());
}

// Verifies that referencing an SPMI target that has a compatible property is rejected.
TEST(SpmiVisitorTest, ReferenceTargetHasCompatibleProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-reference-target-has-compatible-property.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  EXPECT_FALSE(spmi_tester->manager()->Walk(visitors).is_ok());
}

// Verifies that multiple devicetree nodes referencing the same SPMI sub-target is rejected.
TEST(SpmiVisitorTest, TwoNodesReferenceSubTarget) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-two-nodes-reference-sub-target.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  EXPECT_FALSE(spmi_tester->manager()->Walk(visitors).is_ok());
}

// Verifies that referencing an SPMI sub-target that has a compatible property is rejected.
TEST(SpmiVisitorTest, ReferenceSubTargetHasCompatibleProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-reference-has-compatible-property.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  EXPECT_FALSE(spmi_tester->manager()->Walk(visitors).is_ok());
}

// Verifies that an SPMI target with non-SPMI children is parsed as a leaf target.
TEST(SpmiVisitorTest, TargetWithNonSpmiChild) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-target-with-non-spmi-child.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  ASSERT_TRUE(spmi_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(spmi_tester->DoPublish().is_ok());

  // Verify controller metadata is parsed
  auto pbus_node_list = spmi_tester->GetPbusNodes("spmi-abcd0000");
  ASSERT_EQ(1u, pbus_node_list.size());
  const auto& pbus_node = pbus_node_list[0];

  ASSERT_TRUE(pbus_node.metadata());
  ASSERT_EQ(pbus_node.metadata()->size(), 1u);

  const std::vector<uint8_t>& metadata = *(*pbus_node.metadata())[0].data();
  const auto controller =
      fidl::Unpersist<fuchsia_hardware_spmi::ControllerInfo>({metadata.data(), metadata.size()});
  ASSERT_TRUE(controller.is_ok());

  ASSERT_TRUE(controller->id());
  const uint32_t controller_id = *controller->id();

  // Verify the target is parsed, and has NO sub-targets (it is a leaf target in SPMI sense)
  ASSERT_TRUE(controller->targets());
  ASSERT_EQ(controller->targets()->size(), 1u);

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target = FindTargetById(2, *controller);
  ASSERT_TRUE(target);
  EXPECT_FALSE(target->sub_targets());

  // Verify composite node spec for the target itself is added (with TARGETSERVICE)
  const auto target_node_list = spmi_tester->GetCompositeNodeSpecs("i2c-controller-2");
  ASSERT_EQ(1u, target_node_list.size());
  const auto& target_node = target_node_list[0];

  ASSERT_TRUE(target_node.parents2());
  ASSERT_EQ(target_node.parents2()->size(), 2u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 2u),
      },
      (*target_node.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 2u),
      },
      (*target_node.parents2())[1].properties(), false));

  // eeprom@50 has a compatible string so the default BindPropertyVisitor creates a spec for it.
  // Verify that the SPMI visitor did not add any SPMI parents to it.
  const auto eeprom_list = spmi_tester->GetCompositeNodeSpecs("eeprom-50");
  ASSERT_EQ(1u, eeprom_list.size());
  const auto& eeprom = eeprom_list[0];
  ASSERT_TRUE(eeprom.parents2());
  ASSERT_EQ(1u, eeprom.parents2()->size());
}

// Verifies that an SPMI target with multiple SIDs (multi-reg) creates specs for each SID.
TEST(SpmiVisitorTest, MultiRegTarget) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester = new SpmiVisitorTester("/pkg/test-data/spmi-multi-reg.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  ASSERT_TRUE(spmi_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(spmi_tester->DoPublish().is_ok());

  // Controller metadata
  auto pbus_node_list = spmi_tester->GetPbusNodes("spmi-abcd0000");
  ASSERT_EQ(1u, pbus_node_list.size());
  const auto& pbus_node = pbus_node_list[0];

  ASSERT_TRUE(pbus_node.metadata());
  ASSERT_EQ(pbus_node.metadata()->size(), 1u);

  const std::vector<uint8_t>& metadata = *(*pbus_node.metadata())[0].data();
  const auto controller =
      fidl::Unpersist<fuchsia_hardware_spmi::ControllerInfo>({metadata.data(), metadata.size()});
  ASSERT_TRUE(controller.is_ok());

  ASSERT_TRUE(controller->id());
  const uint32_t controller_id = *controller->id();

  ASSERT_TRUE(controller->targets());
  // We expect 4 targets now: 0, 1 from target-a, and 3, 4 from target-b.
  ASSERT_EQ(controller->targets()->size(), 4u);

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_0 = FindTargetById(0, *controller);
  ASSERT_TRUE(target_0);
  EXPECT_EQ(*target_0->name(), "pmic-a");

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_1 = FindTargetById(1, *controller);
  ASSERT_TRUE(target_1);
  EXPECT_EQ(*target_1->name(), "pmic-b");

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_3 = FindTargetById(3, *controller);
  ASSERT_TRUE(target_3);
  EXPECT_EQ(*target_3->name(), "vreg-1");

  const std::optional<fuchsia_hardware_spmi::TargetInfo> target_4 = FindTargetById(4, *controller);
  ASSERT_TRUE(target_4);
  EXPECT_EQ(*target_4->name(), "vreg-2");

  // target-b has no children, so it should have composite node spec.
  // Since target-b has multiple SIDs (3 and 4), it should have separate parent specs for each.
  auto target_b_list = spmi_tester->GetCompositeNodeSpecs("target-b-3");
  ASSERT_EQ(1u, target_b_list.size());
  const auto& target_b = target_b_list[0];

  ASSERT_TRUE(target_b.parents2());
  // 1 parent from default visitor (compatible) + 2 parents from SPMI (one for each SID) = 3
  // parents.
  ASSERT_EQ(target_b.parents2()->size(), 3u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 3u),
      },
      (*target_b.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 3u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "vreg-1"),
      },
      (*target_b.parents2())[1].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 4u),
      },
      (*target_b.parents2())[2].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 4u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "vreg-2"),
      },
      (*target_b.parents2())[2].properties(), false));

  // target-a has no children, so it should have composite node spec.
  // Since target-a has multiple SIDs (0 and 1), it should have separate parent specs for each.
  auto target_a_list = spmi_tester->GetCompositeNodeSpecs("target-a-0");
  ASSERT_EQ(1u, target_a_list.size());
  const auto& target_a = target_a_list[0];

  ASSERT_TRUE(target_a.parents2());
  // 1 parent from default visitor (compatible) + 2 parents from SPMI (one for each SID) = 3
  // parents.
  ASSERT_EQ(target_a.parents2()->size(), 3u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u),
      },
      (*target_a.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "pmic-a"),
      },
      (*target_a.parents2())[1].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 1u),
      },
      (*target_a.parents2())[2].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
          fdf::MakeProperty2(bind_fuchsia::ID, 1u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "pmic-b"),
      },
      (*target_a.parents2())[2].properties(), false));
}

// Verifies that an SPMI target with multiple SIDs and children is rejected.
TEST(SpmiVisitorTest, MultiRegTargetWithChildFail) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  SpmiVisitorTester* const spmi_tester =
      new SpmiVisitorTester("/pkg/test-data/spmi-multi-reg-with-child.dtb");
  ASSERT_TRUE(visitors.RegisterVisitor(std::unique_ptr<SpmiVisitorTester>{spmi_tester}).is_ok());

  EXPECT_FALSE(spmi_tester->manager()->Walk(visitors).is_ok());
}

}  // namespace spmi_dt
