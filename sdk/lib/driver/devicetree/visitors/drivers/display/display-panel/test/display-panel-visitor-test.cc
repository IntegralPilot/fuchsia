// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../display-panel-visitor.h"

#include <lib/device-protocol/display-panel.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <gtest/gtest.h>

#include "dts/display-panel-test.h"
namespace display_panel_visitor_dt {

class DisplayPanelVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<DisplayPanelVisitor> {
 public:
  DisplayPanelVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<DisplayPanelVisitor>(
            dtb_path, "DisplayPanelVisitorTest") {}
};

TEST(DisplayPanelVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<DisplayPanelVisitorTester>("/pkg/test-data/display-panel.dtb");
  DisplayPanelVisitorTester* display_panel_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, display_panel_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(display_panel_visitor_tester->DoPublish().is_ok());

  auto hdmi_node = display_panel_visitor_tester->GetPbusNodes("display");
  ASSERT_EQ(hdmi_node.size(), 1u);
  auto metadata = hdmi_node[0].metadata();
  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  // Controller metadata.
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  display::PanelType* panel_type = reinterpret_cast<display::PanelType*>(metadata_blob.data());
  EXPECT_EQ(static_cast<uint32_t>(*panel_type), static_cast<uint32_t>(TEST_PANEL_TYPE));
}

}  // namespace display_panel_visitor_dt
