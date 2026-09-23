// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../dwc2-visitor.h"

#include <fidl/fuchsia.hardware.usb.dwc2/cpp/fidl.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <gtest/gtest.h>

#include "dts/dwc2-test.h"

namespace dwc2_visitor_dt {

class Dwc2VisitorTester : public fdf_devicetree::testing::VisitorTestHelper<Dwc2Visitor> {
 public:
  Dwc2VisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<Dwc2Visitor>(dtb_path, "Dwc2VisitorTest") {}
};

TEST(Dwc2VisitorTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<Dwc2VisitorTester>("/pkg/test-data/dwc2.dtb");
  Dwc2VisitorTester* dwc2_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, dwc2_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(dwc2_visitor_tester->DoPublish().is_ok());

  auto nodes = dwc2_visitor_tester->GetPbusNodes("usb-ff400000");
  ASSERT_EQ(1lu, nodes.size());
  auto& node = nodes[0];
  auto metadata = node.metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  // Dwc2 metadata
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result dwc2_metadata = fidl::Unpersist<fuchsia_hardware_usb_dwc2::Metadata>(metadata_blob);
  ASSERT_TRUE(dwc2_metadata.is_ok());
  EXPECT_EQ(dwc2_metadata->rx_fifo_size(), static_cast<uint32_t>(TEST_G_RX_FIFO_SIZE));
  EXPECT_EQ(dwc2_metadata->nptx_fifo_size(), static_cast<uint32_t>(TEST_G_NP_TX_FIFO_SIZE));
  EXPECT_EQ(dwc2_metadata->tx_fifo_sizes()[0], static_cast<uint32_t>(TEST_G_TX_FIFO_SIZE_0));
  EXPECT_EQ(dwc2_metadata->tx_fifo_sizes()[1], static_cast<uint32_t>(TEST_G_TX_FIFO_SIZE_1));
  EXPECT_EQ(dwc2_metadata->usb_turnaround_time(), static_cast<uint32_t>(TEST_G_TURNAROUND_TIME));
  EXPECT_EQ(static_cast<uint32_t>(dwc2_metadata->dma_burst_len()),
            static_cast<uint32_t>(TEST_DMA_BURST_LENGTH));
}

}  // namespace dwc2_visitor_dt
