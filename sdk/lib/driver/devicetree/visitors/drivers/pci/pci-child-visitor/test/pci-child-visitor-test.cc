// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/visitors/drivers/pci/pci-child-visitor/pci-child-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pci/cpp/bind.h>
#include <gtest/gtest.h>

namespace pci_child_dt {

class PciChildVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<PciChildVisitor> {
 public:
  explicit PciChildVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<PciChildVisitor>(dtb_path,
                                                                    "PciChildVisitorTest") {}
};

TEST(PciChildVisitorTest, TestPciChildren) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  PciChildVisitorTester* pci_tester = nullptr;
  {
    auto tester = std::make_unique<PciChildVisitorTester>("/pkg/test-data/pci.dtb");
    pci_tester = tester.get();
    ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());
  }

  ASSERT_EQ(ZX_OK, pci_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(pci_tester->DoPublish().is_ok());

  // device@1,0 -> PCI device 00:01.0, PCI_TOPO 0x08, no pci-id override.
  {
    auto specs = pci_tester->GetCompositeNodeSpecs("device-1-0");
    ASSERT_EQ(1lu, specs.size());
    const auto& spec = specs[0];

    ASSERT_TRUE(spec.parents2());
    // Parent 0 is the board driver (self) parent; parent 1 is the PCI fragment.
    ASSERT_EQ(spec.parents2()->size(), 2ul);

    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::PCI_TOPO, 0x08u),
            fdf::MakeAcceptBindRule(bind_fuchsia_pci::SEGMENT, 0u),
        }},
        (*spec.parents2())[1].bind_rules(), false));

    EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
        {{
            fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
        }},
        (*spec.parents2())[1].properties(), false));
  }

  // device@2,0 -> PCI device 00:02.0, PCI_TOPO 0x10, pci-id = <0x8086 0x15f2>.
  {
    auto specs = pci_tester->GetCompositeNodeSpecs("device-2-0");
    ASSERT_EQ(1lu, specs.size());
    const auto& spec = specs[0];

    ASSERT_TRUE(spec.parents2());
    ASSERT_EQ(spec.parents2()->size(), 2ul);

    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::PCI_TOPO, 0x10u),
            fdf::MakeAcceptBindRule(bind_fuchsia_pci::SEGMENT, 0u),
        }},
        (*spec.parents2())[1].bind_rules(), false));

    // The vendor/device id from `pci-id` are advertised as properties so the
    // child can bind a driver by id.
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
        {{
            fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
            fdf::MakeProperty2(bind_fuchsia::PCI_VID, 0x8086u),
            fdf::MakeProperty2(bind_fuchsia::PCI_DID, 0x15f2u),
        }},
        (*spec.parents2())[1].properties(), false));
  }

  // The "fuchsia,config" child has no `reg` and must be ignored.
  EXPECT_EQ(0lu, pci_tester->GetCompositeNodeSpecs("config").size());

  // device@3,0 lives under the second host bridge, which declares
  // `fuchsia,pci-domain = <1>` -> PCI device 0001:00:03.0, PCI_TOPO 0x18.
  {
    auto specs = pci_tester->GetCompositeNodeSpecs("device-3-0");
    ASSERT_EQ(1lu, specs.size());
    const auto& spec = specs[0];

    ASSERT_TRUE(spec.parents2());
    ASSERT_EQ(spec.parents2()->size(), 2ul);

    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::PCI_TOPO, 0x18u),
            fdf::MakeAcceptBindRule(bind_fuchsia_pci::SEGMENT, 1u),
        }},
        (*spec.parents2())[1].bind_rules(), false));
  }

  // device@0,0 is behind a PCI-to-PCI bridge and inherits the host bridge's
  // domain -> PCI device 0001:01:00.0, PCI_TOPO 0x100.
  {
    auto specs = pci_tester->GetCompositeNodeSpecs("device-0-0");
    ASSERT_EQ(1lu, specs.size());
    const auto& spec = specs[0];

    ASSERT_TRUE(spec.parents2());
    ASSERT_EQ(spec.parents2()->size(), 2ul);

    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
            fdf::MakeAcceptBindRule(bind_fuchsia::PCI_TOPO, 0x100u),
            fdf::MakeAcceptBindRule(bind_fuchsia_pci::SEGMENT, 1u),
        }},
        (*spec.parents2())[1].bind_rules(), false));
  }

  // The visitor exposes the BDFs it wired up (in devicetree order) so a board
  // driver can forward them to the bus driver as devicetree_bdfs. Each BDF is
  // qualified by the domain of the bus it is on: 0 for the host bridge that
  // declares no `fuchsia,pci-domain`, and 1 for the one that declares it.
  const std::vector<PciChildBdf>& bdfs = pci_tester->child_bdfs();
  ASSERT_EQ(5lu, bdfs.size());
  EXPECT_EQ(bdfs[0].domain, 0x0000u);
  EXPECT_EQ(bdfs[0].bus, 0x00u);
  EXPECT_EQ(bdfs[0].device, 0x01u);
  EXPECT_EQ(bdfs[0].function, 0x00u);
  EXPECT_EQ(bdfs[1].domain, 0x0000u);
  EXPECT_EQ(bdfs[1].bus, 0x00u);
  EXPECT_EQ(bdfs[1].device, 0x02u);
  EXPECT_EQ(bdfs[1].function, 0x00u);
  EXPECT_EQ(bdfs[2].domain, 0x0001u);
  EXPECT_EQ(bdfs[2].bus, 0x00u);
  EXPECT_EQ(bdfs[2].device, 0x03u);
  EXPECT_EQ(bdfs[2].function, 0x00u);
  // The bridge itself is a PCI device on the host bridge's bus.
  EXPECT_EQ(bdfs[3].domain, 0x0001u);
  EXPECT_EQ(bdfs[3].bus, 0x00u);
  EXPECT_EQ(bdfs[3].device, 0x04u);
  EXPECT_EQ(bdfs[3].function, 0x00u);
  // The device behind the bridge inherits the host bridge's domain.
  EXPECT_EQ(bdfs[4].domain, 0x0001u);
  EXPECT_EQ(bdfs[4].bus, 0x01u);
  EXPECT_EQ(bdfs[4].device, 0x00u);
  EXPECT_EQ(bdfs[4].function, 0x00u);
  EXPECT_EQ(0lu, pci_tester->GetPbusNodes("device-").size());
}

TEST(PciChildVisitorTest, InvalidPciDomain) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<PciChildVisitorTester>("/pkg/test-data/pci-invalid-domain.dtb");
  PciChildVisitorTester* pci_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_ERR_INTERNAL, pci_tester->manager()->Walk(visitors).status_value());
}

}  // namespace pci_child_dt
