// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/manager/manager-test-helper.h>
#include <lib/driver/devicetree/manager/manager.h>
#include <lib/driver/devicetree/manager/publisher-host.h>
#include <lib/driver/devicetree/visitors/default/default.h>

#include <string>

#include <gtest/gtest.h>

#ifndef TEST_DATA_DIR
#error TEST_DATA_DIR must be defined
#endif

namespace fdf_devicetree {

class ManagerHostTest : public fdf_devicetree::testing::ManagerTestHelper, public ::testing::Test {
 public:
  ManagerHostTest()
      : fdf_devicetree::testing::ManagerTestHelper(fdf_devicetree::testing::CreateTestPublisher()) {
  }
};

TEST_F(ManagerHostTest, SimpleDtbTest) {
  std::string dtb_path = std::string(TEST_DATA_DIR) + "/simple.dtb";
  std::vector<uint8_t> data = fdf_devicetree::testing::LoadTestBlob(dtb_path.c_str());
  Manager manager(std::move(data));
  DefaultVisitors<> default_visitors;

  ASSERT_EQ(ZX_OK, manager.Walk(default_visitors).status_value());
  ASSERT_EQ(ZX_OK, this->DoPublish(manager).status_value());

  auto host_publisher = static_cast<PublisherHost*>(this->publisher());

  EXPECT_EQ(0lu, host_publisher->GetPbusNodes().size());
  EXPECT_EQ(0lu, host_publisher->GetPbusNodesWithMetadata().size());
  EXPECT_EQ(2lu, host_publisher->GetBoardChildNodes().size());
  EXPECT_EQ(2lu, host_publisher->GetCompositeNodeSpecs().size());
  EXPECT_EQ(0lu, host_publisher->GetIommus().size());

  if (host_publisher->GetBoardChildNodes().size() >= 2) {
    EXPECT_EQ("dt-root", host_publisher->GetBoardChildNodes()[0].name);
    EXPECT_TRUE(host_publisher->GetBoardChildNodes()[1].name.find("example-device") !=
                std::string::npos);
  }
}

TEST_F(ManagerHostTest, BasicPropertiesTest) {
  std::string dtb_path = std::string(TEST_DATA_DIR) + "/basic-properties.dtb";
  std::vector<uint8_t> data = fdf_devicetree::testing::LoadTestBlob(dtb_path.c_str());
  Manager manager(std::move(data));
  DefaultVisitors<> default_visitors;

  ASSERT_EQ(ZX_OK, manager.Walk(default_visitors).status_value());
  ASSERT_EQ(ZX_OK, this->DoPublish(manager).status_value());

  auto host_publisher = static_cast<PublisherHost*>(this->publisher());

  // In BasicPropertiesTest, sample-device@0 has reg, so it should be a platform device
  // because of the Mmio visitor.
  EXPECT_EQ(1lu, host_publisher->GetPbusNodes().size());
  EXPECT_EQ(10lu, host_publisher->GetBoardChildNodes().size());
}

}  // namespace fdf_devicetree
