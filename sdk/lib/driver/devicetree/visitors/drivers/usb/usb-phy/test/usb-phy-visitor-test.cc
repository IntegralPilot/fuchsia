// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../usb-phy-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/usb/phy/cpp/bind.h>
#include <gtest/gtest.h>
namespace usb_phy_visitor_dt {

class UsbPhyVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<UsbPhyVisitor> {
 public:
  UsbPhyVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<UsbPhyVisitor>(dtb_path, "UsbPhyVisitorTest") {}
};

TEST(UsbVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<UsbPhyVisitorTester>("/pkg/test-data/usb-phy.dtb");
  UsbPhyVisitorTester* usb_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, usb_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(usb_visitor_tester->DoPublish().is_ok());

  auto nodes = usb_visitor_tester->GetPbusNodes("test");
  ASSERT_EQ(1lu, nodes.size());

  ASSERT_EQ(4lu, usb_visitor_tester->GetCompositeNodeSpecs().size());
  auto mgr_requests = usb_visitor_tester->GetCompositeNodeSpecs(*nodes[0].name());
  ASSERT_EQ(1lu, mgr_requests.size());
  auto mgr_request = mgr_requests[0];
  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(3lu, mgr_request.parents2()->size());

  // 1st parent is pdev. Skip that.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                                bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                                bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                                bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_XHCI)}},
      (*mgr_request.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, "xhci"),
          fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                             bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC),
          fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                             bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
          fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                             bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_XHCI),
      }},
      (*mgr_request.parents2())[1].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_usb_phy::NAME, "another-phy"),
      }},
      (*mgr_request.parents2())[2].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
          fdf::MakeProperty2(bind_fuchsia_usb_phy::NAME, "another-phy"),
      }},
      (*mgr_request.parents2())[2].properties(), false));
}

}  // namespace usb_phy_visitor_dt
