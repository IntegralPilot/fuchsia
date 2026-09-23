// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../my-devicetree-visitor.h"

#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <gtest/gtest.h>
namespace my_devicetree_visitor_dt {

class MyDevicetreeVisitorTester : public
    fdf_devicetree::testing::VisitorTestHelper<MyDevicetreeVisitor> {
 public:
  MyDevicetreeVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<MyDevicetreeVisitor>(
            dtb_path, "MyDevicetreeVisitorTest") {
  }
};

TEST(MyDevicetreeVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<MyDevicetreeVisitorTester>("/pkg/test-data/my-devicetree-visitor.dtb");
  MyDevicetreeVisitorTester* my_devicetree_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, my_devicetree_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(my_devicetree_visitor_tester->DoPublish().is_ok());

  // <Add tests for specific node properties>.
  auto nodes = my_devicetree_visitor_tester->GetBoardChildNodes("sample-device");
  ASSERT_EQ(nodes.size(), 1u);

  // <Add Node specific metadata or bind property tests below>.
}

}  // namespace my_devicetree_visitor_dt
