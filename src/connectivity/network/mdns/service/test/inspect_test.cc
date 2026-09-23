// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/inspect.h"

#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace mdns::testing {

// Inspect Node names
constexpr std::string kNode_Time = "@time";
constexpr std::string kNode_MdnsStatus = "mdns_status";
constexpr std::string kNode_MdnsStatus_State = "state";

class MdnsInspectorTest : public ::gtest::RealLoopFixture {
 public:
  MdnsInspectorTest() : mdns_inspector_(loop().dispatcher()) {}
  // Get the underlying inspect::Inspector for Mdns component.
  inspect::Inspector* GetInspector() {
    auto& mdns_inspector = GetTestMdnsInspector();
    return &mdns_inspector.inspector_->inspector();
  }

  // Return MdnsInspector for test.
  MdnsInspector& GetTestMdnsInspector() { return mdns_inspector_; }

  // Validate single Mdn status entry node.
  void ValidateMdnsStatusEntryNode(
      const inspect::Hierarchy* mdns_status_node,
      MdnsInspector::MdnsStatusNode::MdnsCurrentStatus mdns_status_info) {
    ASSERT_TRUE(mdns_status_node);

    auto* timestamp = mdns_status_node->node().get_property<inspect::UintPropertyValue>(kNode_Time);
    ASSERT_TRUE(timestamp);

    auto* state =
        mdns_status_node->node().get_property<inspect::StringPropertyValue>(kNode_MdnsStatus_State);
    ASSERT_TRUE(state);
    EXPECT_EQ(state->value(), mdns_status_info.state_);
  }

  // Returns Mdns status info filled with init values.
  MdnsInspector::MdnsStatusNode::MdnsCurrentStatus GetInitializedMdnsStatusInfo() {
    MdnsInspector::MdnsStatusNode::MdnsCurrentStatus mdns_status_info;
    mdns_status_info.state_ = MdnsInspector::kNotStarted;
    return mdns_status_info;
  }

 private:
  MdnsInspector mdns_inspector_;
};

TEST_F(MdnsInspectorTest, NotifyMdnsStatusChanges) {
  auto status = GetInitializedMdnsStatusInfo();
  auto& mdns_inspector = GetTestMdnsInspector();

  mdns_inspector.NotifyStateChange(MdnsInspector::kWaitingForInterfaces);
  mdns_inspector.NotifyStateChange(MdnsInspector::kAddressProbeInProgress);

  fpromise::result<inspect::Hierarchy> hierarchy =
      RunPromise(inspect::ReadFromInspector(*GetInspector()));
  ASSERT_TRUE(hierarchy.is_ok());

  auto* mdns_status = hierarchy.value().GetByPath({kNode_MdnsStatus});
  ASSERT_TRUE(mdns_status);

  auto* mdns_status_node_0 = mdns_status->GetByPath({"0x0"});
  status.state_ = MdnsInspector::kWaitingForInterfaces;
  ValidateMdnsStatusEntryNode(mdns_status_node_0, status);

  auto* mdns_status_node_1 = mdns_status->GetByPath({"0x1"});
  status.state_ = MdnsInspector::kAddressProbeInProgress;
  ValidateMdnsStatusEntryNode(mdns_status_node_1, status);
}

}  // namespace mdns::testing
