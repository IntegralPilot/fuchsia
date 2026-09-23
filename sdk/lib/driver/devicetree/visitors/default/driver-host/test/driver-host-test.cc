// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/driver-host/driver-host.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <gtest/gtest.h>

namespace fdf_devicetree {
namespace {

class DriverHostVisitorTester : public testing::VisitorTestHelper<DriverHostVisitor> {
 public:
  DriverHostVisitorTester(std::string_view dtb_path)
      : VisitorTestHelper<DriverHostVisitor>(dtb_path, "DriverHostVisitorTest") {}
};

TEST(DriverHostVisitorTest, TestDriverHostProperty) {
  VisitorRegistry visitors;
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<DriverHostVisitorTester>("/pkg/test-data/driver-host.dtb");
  DriverHostVisitorTester* driver_host_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, driver_host_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(driver_host_tester->DoPublish().is_ok());

  std::vector<BoardChildNode> nodes = driver_host_tester->GetBoardChildNodes("sample-device");
  ASSERT_EQ(1lu, nodes.size());
  ASSERT_EQ(nodes[0].driver_host, "#samples");
}

}  // namespace
}  // namespace fdf_devicetree
