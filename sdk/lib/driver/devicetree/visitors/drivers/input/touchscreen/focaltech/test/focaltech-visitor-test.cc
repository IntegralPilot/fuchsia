// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../focaltech-visitor.h"

#include <fidl/fuchsia.hardware.input.focaltech/cpp/fidl.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <gtest/gtest.h>
namespace focaltech_visitor_dt {

class FocaltechVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<FocaltechVisitor> {
 public:
  FocaltechVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<FocaltechVisitor>(dtb_path,
                                                                     "FocaltechVisitorTest") {}
};

TEST(FocaltechVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<FocaltechVisitorTester>("/pkg/test-data/focaltech.dtb");
  FocaltechVisitorTester* focaltech_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, focaltech_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(focaltech_visitor_tester->DoPublish().is_ok());

  std::vector<fuchsia_hardware_platform_bus::Node> touch_nodes =
      focaltech_visitor_tester->GetPbusNodes("touchscreen");
  ASSERT_EQ(1lu, touch_nodes.size());
  auto metadata = touch_nodes[0].metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());

  fit::result device_info =
      fidl::Unpersist<fuchsia_hardware_input_focaltech::Metadata>(metadata_blob);
  ASSERT_TRUE(device_info.is_ok());
  EXPECT_EQ(device_info->device_id(), fuchsia_hardware_input_focaltech::DeviceId::kFt6336);
  EXPECT_TRUE(device_info->needs_firmware());
}

}  // namespace focaltech_visitor_dt
