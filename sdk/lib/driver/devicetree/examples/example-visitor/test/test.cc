// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <gtest/gtest.h>

#include "../example-visitor.h"

namespace example {

namespace {

class ExampleVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<ExampleDriverVisitor> {
 public:
  explicit ExampleVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<ExampleDriverVisitor>(dtb_path,
                                                                         "ExampleVisitorTest") {}
};

TEST(ExampleVisitorTest, TestVisitorWithDtb) {
  // Create a registry to hold all the visitors.
  fdf_devicetree::VisitorRegistry visitors;

  // Register standard visitors required for compatible string and MMIO handling.
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  // Create an instance of the ExampleVisitorTester. This class inherits from
  // VisitorTestHelper and allows us to test our ExampleDriverVisitor.
  auto tester = std::make_unique<ExampleVisitorTester>(DTB_PATH);
  auto* tester_ptr = tester.get();

  // Add ExampleVisitorTester to the registry.
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  // Walk the device tree using the visitors.
  auto result = tester_ptr->manager()->Walk(visitors);
  ASSERT_TRUE(result.is_ok()) << "Walk failed";

  // Publish the metadata and nodes collected by the visitors.
  ASSERT_TRUE(tester_ptr->DoPublish().is_ok());

  // Retrieve the PbusNodes added for the "sample-device".
  auto nodes = tester_ptr->GetPbusNodes("sample-device");

  // Expect exactly one node to be added for "sample-device".
  ASSERT_EQ(nodes.size(), 1lu);

  // Check if the ExampleDriverVisitor's Visit method was called.
  ASSERT_TRUE(tester_ptr->has_visited());
}

}  // namespace

}  // namespace example
