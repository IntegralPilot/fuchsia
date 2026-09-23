// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../amlogic-canvas-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>
namespace amlogic_canvas_dt {

class AmlogicCanvasVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<AmlogicCanvasVisitor> {
 public:
  AmlogicCanvasVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<AmlogicCanvasVisitor>(
            dtb_path, "AmlogicCanvasVisitorTest") {}
};

TEST(AmlogicCanvasVisitorTest, TestBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<AmlogicCanvasVisitorTester>("/pkg/test-data/amlogic-canvas.dtb");
  AmlogicCanvasVisitorTester* canvas_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, canvas_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(canvas_tester->DoPublish().is_ok());

  ASSERT_EQ(1lu, canvas_tester->GetCompositeNodeSpecs("video-decoder").size());
  auto mgr_request = canvas_tester->GetCompositeNodeSpecs("video-decoder")[0];

  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service")}},
      (*mgr_request.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
      }},
      (*mgr_request.parents2())[1].properties(), false));
}

}  // namespace amlogic_canvas_dt
