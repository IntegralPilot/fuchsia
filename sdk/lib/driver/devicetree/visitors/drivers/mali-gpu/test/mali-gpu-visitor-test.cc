// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../mali-gpu-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>
namespace mali_gpu_dt {

class MaliGpuVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<MaliGpuVisitor> {
 public:
  MaliGpuVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<MaliGpuVisitor>(dtb_path, "MaliGpuVisitorTest") {
  }
};

TEST(MaliGpuVisitorTest, TestBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<MaliGpuVisitorTester>("/pkg/test-data/mali-gpu.dtb");
  MaliGpuVisitorTester* mali_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, mali_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(mali_tester->DoPublish().is_ok());

  auto mgr_request_list = mali_tester->GetCompositeNodeSpecs("mali-controller");
  ASSERT_EQ(1lu, mgr_request_list.size());
  auto mgr_request = mgr_request_list[0];

  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpu.mali.Service")}},
      (*mgr_request.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpu.mali.Service"),
      }},
      (*mgr_request.parents2())[1].properties(), false));
}

}  // namespace mali_gpu_dt
