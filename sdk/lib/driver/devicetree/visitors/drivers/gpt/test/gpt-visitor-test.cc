// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/visitors/drivers/gpt/gpt-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <optional>

#include <bind/fuchsia/block/gpt/cpp/bind.h>
#include <gtest/gtest.h>

namespace gpt_dt {

class GptVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<GptVisitor> {
 public:
  explicit GptVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<GptVisitor>(dtb_path, "GptVisitorTest") {}
};

TEST(GptVisitorTest, TestGptProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  auto tester = std::make_unique<GptVisitorTester>("/pkg/test-data/test-dt.dtb");
  GptVisitorTester* gpt_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, gpt_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(gpt_tester->DoPublish().is_ok());

  ASSERT_EQ(1lu, gpt_tester->GetCompositeNodeSpecs().size());
  auto mgr_request = gpt_tester->GetCompositeNodeSpecs()[0];
  ASSERT_TRUE(mgr_request.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request.parents2()->size());

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia_block_gpt::PARTITION_NAME, "partition1"),
      }},
      (*mgr_request.parents2())[0].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia_block_gpt::PARTITION_NAME, "partition1"),
      }},
      (*mgr_request.parents2())[0].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia_block_gpt::PARTITION_NAME, "partition2"),
      }},
      (*mgr_request.parents2())[1].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia_block_gpt::PARTITION_NAME, "partition2"),
      }},
      (*mgr_request.parents2())[1].properties(), false));
}

}  // namespace gpt_dt
