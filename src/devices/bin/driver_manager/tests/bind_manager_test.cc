// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/tests/bind_manager_test_base.h"

class BindManagerTest : public BindManagerTestBase {};

TEST_F(BindManagerTest, BindSingleNode) {
  AddAndOrphanNode("node-a");

  // Invoke TryBindAllAvailable() in the bind manager.
  InvokeTryBindAllAvailable_EXPECT_BIND_START();
  VerifyBindOngoingWithRequests({{"node-a", 1}});

  // Invoke a driver match response from the Driver Index.
  // The node shouldn't be orphaned and bind should end.
  DriverIndexReplyWithDriver("node-a");
  VerifyOrphanedNodes({});

  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, TryBindAllAvailableWithNoNodes) {
  InvokeTryBindAllAvailable();
  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, NonOverlappingRequests) {
  AddAndOrphanNode("node-a");
  AddAndOrphanNode("node-b");

  // Invoke TryBindAllAvailable() in the bind manager.
  InvokeTryBindAllAvailable_EXPECT_BIND_START();
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}});

  // Driver index completes the request with a match for node-a. We should have one
  // request left.
  DriverIndexReplyWithDriver("node-a");
  VerifyBindOngoingWithRequests({{"node-b", 1}});

  // Driver index completes the request with no matches for node-b.
  DriverIndexReplyWithNoMatch("node-b");
  VerifyOrphanedNodes({"node-b"});

  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, OverlappingBindRequests) {
  // Invoke bind for a new node in the bind manager.
  AddAndBindNode_EXPECT_BIND_START("node-a");
  VerifyBindOngoingWithRequests({{"node-a", 1}});

  // Add and invoke bind for two more nodes while bind is ongoing. The requests should
  // be queued.
  AddAndBindNode_EXPECT_QUEUED("node-b");
  AddAndBindNode_EXPECT_QUEUED("node-c");

  // Complete the ongoing bind process.
  DriverIndexReplyWithDriver("node-a");
  VerifyOrphanedNodes({});

  // The queued requests should be processed and kickstart a new bind process.
  VerifyBindOngoingWithRequests({{"node-b", 1}, {"node-c", 1}});
  VerifyNoQueuedBind();

  // Complete the second ongoing bind process.
  DriverIndexReplyWithDriver("node-b");
  DriverIndexReplyWithNoMatch("node-c");
  VerifyOrphanedNodes({"node-c"});

  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, BindNodeOverlapTryAllAvailable) {
  AddAndOrphanNode("node-a");
  AddAndOrphanNode("node-b");
  AddAndOrphanNode("node-c");

  InvokeTryBindAllAvailable_EXPECT_BIND_START();
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}, {"node-c", 1}});

  AddAndBindNode_EXPECT_QUEUED("node-d");
  AddAndBindNode_EXPECT_QUEUED("node-e");

  // Complete the ongoing bind process.
  DriverIndexReplyWithDriver("node-b");
  DriverIndexReplyWithDriver("node-a");
  DriverIndexReplyWithNoMatch("node-c");
  VerifyOrphanedNodes({"node-c"});

  // Verify that the queued requests are processed.
  VerifyBindOngoingWithRequests({{"node-d", 1}, {"node-e", 1}});
  VerifyNoQueuedBind();

  // Complete the ongoing bind. With no matches, node-d and node-e should added to the
  // orphaned nodes.
  DriverIndexReplyWithNoMatch("node-d");
  DriverIndexReplyWithNoMatch("node-e");
  VerifyOrphanedNodes({"node-c", "node-d", "node-e"});
  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, OverlappingTryAllAvailable) {
  AddAndOrphanNode("node-a");
  AddAndOrphanNode("node-b");
  AddAndOrphanNode("node-c");

  // Invoke TryBindAllAvailable().
  InvokeTryBindAllAvailable_EXPECT_BIND_START();
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}, {"node-c", 1}});

  // Invoke TryBindAllAvailable() twice while there's an ongoing bind process. They
  // should be queued.
  InvokeTryBindAllAvailable_EXPECT_QUEUED();
  InvokeTryBindAllAvailable_EXPECT_QUEUED();

  // Match the next two nodes.
  DriverIndexReplyWithNoMatch("node-b");
  DriverIndexReplyWithNoMatch("node-a");

  // Match the final node in the ongoing bind process. This should kickstart a new
  // bind process with the queued bind requests.
  DriverIndexReplyWithDriver("node-c");
  VerifyOrphanedNodes({"node-a", "node-b"});

  // Verify that the TryBindAllAvailable() request is processed with the two orphaned
  // nodes.
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}});
  VerifyNoQueuedBind();

  // Complete the ongoing bind. Since the queued TryBindAllAvailable() calls are
  // consolidated, there shouldn't be a follow up bind process.
  DriverIndexReplyWithNoMatch("node-a");
  DriverIndexReplyWithNoMatch("node-b");
  VerifyOrphanedNodes({"node-a", "node-b"});
  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, TryAllAvailableOverBind) {
  // Invoke bind for a new node in the bind manager.
  AddAndBindNode_EXPECT_BIND_START("node-a");
  VerifyBindOngoingWithRequests({{"node-a", 1}});

  // Invoke TryBindAllAvailable() twice. Both requests should be queued.
  InvokeTryBindAllAvailable_EXPECT_QUEUED();
  InvokeTryBindAllAvailable_EXPECT_QUEUED();

  // Complete the ongoing bind process. This should kickstart a new bind process.
  DriverIndexReplyWithNoMatch("node-a");
  VerifyBindOngoingWithRequests({{"node-a", 1}});

  // Complete the new ongoing bind process. Since the queued TryBindAllAvailable() calls
  // are consolidated, there shouldn't be a follow up bind process.
  DriverIndexReplyWithDriver("node-a");
  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, OverlappingBindWithSameNode) {
  // Kickstart bind with node a.
  AddAndBindNode_EXPECT_BIND_START("node-a");
  VerifyBindOngoingWithRequests({{"node-a", 1}});

  // Queue a couple of bind requests that involves node-a.
  InvokeTryBindAllAvailable_EXPECT_QUEUED();
  InvokeBind_EXPECT_QUEUED("node-a");

  // Complete the ongoing bind process. Node-a should be bound.
  DriverIndexReplyWithDriver("node-a");

  // Since we have a pending bind request for node-a, we should make another attempt
  // to bind.
  VerifyBindOngoingWithRequests({{"node-a", 1}});
  DriverIndexReplyWithDriver("node-a");

  VerifyNoOngoingBind();
  VerifyOrphanedNodes({});
}

TEST_F(BindManagerTest, PendingBindShareSameNode) {
  AddAndOrphanNode("node-a");

  // Kickstart bind with node-b.
  AddAndBindNode_EXPECT_BIND_START("node-b");
  VerifyBindOngoingWithRequests({{"node-b", 1}});

  // Queue TryBindAllAvailable() and two Bind() for node-a.
  InvokeTryBindAllAvailable_EXPECT_QUEUED();
  InvokeBind_EXPECT_QUEUED("node-a");
  InvokeBind_EXPECT_QUEUED("node-b");
  InvokeBind_EXPECT_QUEUED("node-a");

  // Complete the ongoing bind process. It should kickstart another bind process.
  DriverIndexReplyWithNoMatch("node-b");

  // We should have two match requests for node-a since TryBindAllAvailable() should
  // exclude node-a.
  VerifyBindOngoingWithRequests({{"node-a", 2}, {"node-b", 1}});
  DriverIndexReplyWithDriver("node-a");
  DriverIndexReplyWithDriver("node-b");
  DriverIndexReplyWithDriver("node-a");

  VerifyNoOngoingBind();
  VerifyOrphanedNodes({});
}

TEST_F(BindManagerTest, AddCompositeNodeSpecThenBind) {
  AddCompositeNodeSpec("composite-a", {"node-a", "node-b"});

  // Add and bind node-a.
  AddAndBindNode("node-a");
  VerifyBindOngoingWithRequests({{"node-a", 1}});
  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});

  // Add and bind node-b.
  AddAndBindNode("node-b");
  VerifyBindOngoingWithRequests({{"node-b", 1}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}});

  VerifyCompositeNodeExists(true, "composite-a");
  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, AddNodesBetweenAddingSpec) {
  AddAndOrphanNode("node-a");
  VerifyNoOngoingBind();

  // Add the spec. It should kickstart the bind process.
  AddCompositeNodeSpec("composite-a", {"node-a", "node-b"});
  VerifyBindOngoingWithRequests({{"node-a", 1}});

  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  VerifyOrphanedNodes({});
  VerifyNoOngoingBind();

  // Add the remaining node for the spec.
  AddAndBindNode("node-b");
  VerifyBindOngoingWithRequests({{"node-b", 1}});

  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}});
  VerifyCompositeNodeExists(true, "composite-a");
  VerifyOrphanedNodes({});
  VerifyNoOngoingBind();
}

TEST_F(BindManagerTest, AddNodesThenSpec) {
  AddAndOrphanNode("node-a");
  AddAndOrphanNode("node-b");
  VerifyNoOngoingBind();

  AddCompositeNodeSpec("composite-a", {"node-a", "node-b"});
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}});

  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}});
  VerifyCompositeNodeExists(true, "composite-a");

  VerifyOrphanedNodes({});
  VerifyNoOngoingBind();
}
