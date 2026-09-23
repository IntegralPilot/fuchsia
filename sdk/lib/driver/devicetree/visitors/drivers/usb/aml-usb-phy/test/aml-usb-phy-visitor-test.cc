// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../aml-usb-phy-visitor.h"

#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <gtest/gtest.h>
#include <usb/usb.h>

namespace aml_usb_phy_visitor_dt {

class AmlUsbPhyVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<AmlUsbPhyVisitor> {
 public:
  AmlUsbPhyVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<AmlUsbPhyVisitor>(dtb_path,
                                                                     "AmlUsbPhyVisitorTest") {}
};

TEST(AmlUsbPhyVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<AmlUsbPhyVisitorTester>("/pkg/test-data/aml-usb-phy.dtb");
  AmlUsbPhyVisitorTester* aml_usb_phy_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, aml_usb_phy_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(aml_usb_phy_visitor_tester->DoPublish().is_ok());

  auto nodes = aml_usb_phy_visitor_tester->GetPbusNodes("phy-ffe00000");
  ASSERT_EQ(1lu, nodes.size());
  auto& node = nodes[0];
  auto metadata = node.metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  std::vector<uint8_t> metadata_blob_1 = std::move(*(*metadata)[0].data());
  fit::result usb_phy_metadata =
      fidl::Unpersist<fuchsia_hardware_usb_phy::Metadata>(metadata_blob_1);
  ASSERT_TRUE(usb_phy_metadata.is_ok());
  EXPECT_EQ(usb_phy_metadata->phy_type(), fuchsia_hardware_usb_phy::AmlogicPhyType::kG12B);
  const auto& phy_modes = usb_phy_metadata->usb_phy_modes().value();
  ASSERT_EQ(phy_modes.size(), 3lu);
  EXPECT_EQ(phy_modes[0].protocol(), fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20);
  EXPECT_EQ(phy_modes[0].dr_mode(), fuchsia_hardware_usb_phy::Mode::kHost);
  EXPECT_EQ(phy_modes[0].is_otg_capable(), false);
  EXPECT_EQ(phy_modes[1].protocol(), fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20);
  EXPECT_EQ(phy_modes[1].dr_mode(), fuchsia_hardware_usb_phy::Mode::kPeripheral);
  EXPECT_EQ(phy_modes[1].is_otg_capable(), true);
  EXPECT_EQ(phy_modes[2].protocol(), fuchsia_hardware_usb_phy::ProtocolVersion::kUsb30);
  EXPECT_EQ(phy_modes[2].dr_mode(), fuchsia_hardware_usb_phy::Mode::kHost);
  EXPECT_EQ(phy_modes[2].is_otg_capable(), false);
}

}  // namespace aml_usb_phy_visitor_dt
