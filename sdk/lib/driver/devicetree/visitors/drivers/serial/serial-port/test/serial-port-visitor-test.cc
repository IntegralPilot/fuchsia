// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../serial-port-visitor.h"

#include <fidl/fuchsia.hardware.serial/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/serial/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/serial-port-test.h"
namespace serial_port_visitor_dt {

class SerialPortVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<SerialPortVisitor> {
 public:
  SerialPortVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<SerialPortVisitor>(dtb_path,
                                                                      "SerialPortVisitorTest") {}
};

TEST(SerialPortVisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<SerialPortVisitorTester>("/pkg/test-data/serial-port.dtb");
  SerialPortVisitorTester* serial_port_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, serial_port_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(serial_port_visitor_tester->DoPublish().is_ok());

  auto node_count = serial_port_visitor_tester->GetPbusNodes().size();

  uint32_t node_tested_count = 0;
  for (size_t i = 0; i < node_count; i++) {
    auto node = serial_port_visitor_tester->GetPbusNodes()[i];

    if (node.name()->find("bt-uart") != std::string::npos) {
      node_tested_count++;
      auto metadata = serial_port_visitor_tester->GetPbusNodes()[i].metadata();

      // Test metadata properties.
      ASSERT_TRUE(metadata);
      ASSERT_EQ(1lu, metadata->size());

      std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
      fit::result serial_port =
          fidl::Unpersist<fuchsia_hardware_serial::SerialPortInfo>(metadata_blob);
      ASSERT_TRUE(serial_port.is_ok());
      EXPECT_EQ(serial_port->serial_class(),
                static_cast<fuchsia_hardware_serial::Class>(TEST_CLASS));
      EXPECT_EQ(serial_port->serial_vid(), static_cast<uint32_t>(TEST_VID));
      EXPECT_EQ(serial_port->serial_pid(), static_cast<uint32_t>(TEST_PID));
    }
  }

  for (auto& node : serial_port_visitor_tester->GetBoardChildNodes("bt")) {
    node_tested_count++;
    ASSERT_EQ(3lu, serial_port_visitor_tester->GetCompositeNodeSpecs().size());

    auto mgr_requests = serial_port_visitor_tester->GetCompositeNodeSpecs(node.name);
    auto it = std::find_if(mgr_requests.begin(), mgr_requests.end(),
                           [&](const auto& spec) { return *spec.name() == node.name; });
    ASSERT_NE(it, mgr_requests.end());
    auto mgr_request = *it;
    ASSERT_TRUE(mgr_request.parents2().has_value());
    ASSERT_EQ(2lu, mgr_request.parents2()->size());

    // 1st parent is pdev. Skipping that.
    // 2nd parent is bt-uart.
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
        {{
            fdf::MakeProperty2(bind_fuchsia_serial::NAME, TEST_NAME),
            fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.serial.Service"),
        }},
        (*mgr_request.parents2())[1].properties(), false));
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{
            fdf::MakeAcceptBindRule(bind_fuchsia::SERIAL_CLASS, static_cast<uint32_t>(TEST_CLASS)),
            fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.serial.Service"),
            // TODO(https://fxbug.dev/467370573): Temporary workaround for a composite issue.
            // Remove this once the composite issue is resolved.
            fdf::MakeRejectBindRule(bind_fuchsia_serial::NAME, "bt-passthrough-hci"),
        }},
        (*mgr_request.parents2())[1].bind_rules(), false));
  }

  ASSERT_EQ(node_tested_count, 2u);
}

}  // namespace serial_port_visitor_dt
