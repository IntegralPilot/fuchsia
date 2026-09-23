// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../thermal-zones-visitor.h"

#include <fidl/fuchsia.hardware.trippoint/cpp/fidl.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <gtest/gtest.h>

#include "dts/thermal-zones-test.h"
namespace thermal_zones_visitor_dt {

class ThermalZonesVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<ThermalZonesVisitor> {
 public:
  ThermalZonesVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<ThermalZonesVisitor>(
            dtb_path, "ThermalZonesVisitorTest") {}
};

TEST(ThermalZonesVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<ThermalZonesVisitorTester>("/pkg/test-data/thermal-zones.dtb");
  ThermalZonesVisitorTester* thermal_zones_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, thermal_zones_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(thermal_zones_visitor_tester->DoPublish().is_ok());

  auto nodes = thermal_zones_visitor_tester->GetPbusNodes("ddr-sensor");
  ASSERT_EQ(1lu, nodes.size());
  auto& node = nodes[0];
  auto metadata = node.metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  // trippoint metadata.
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result trip_metadata =
      fidl::Unpersist<fuchsia_hardware_trippoint::TripDeviceMetadata>(metadata_blob);
  ASSERT_TRUE(trip_metadata.is_ok());
  ASSERT_EQ((*trip_metadata).critical_temp_celsius(), static_cast<float>(CRITICAL_TEMP) / 1000);
}

}  // namespace thermal_zones_visitor_dt
