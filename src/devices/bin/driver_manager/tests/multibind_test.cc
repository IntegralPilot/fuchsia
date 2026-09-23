// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/tests/bind_manager_test_base.h"

class MultibindTest : public BindManagerTestBase {};

TEST_F(MultibindTest, MultibindSpec_CompositeMultibindDisabled) {
  AddAndOrphanNode("node-a", /* enable_multibind */ false);
  AddAndOrphanNode("node-c", /* enable_multibind */ true);
  VerifyNoOngoingBind();

  // Add composite-a. Since node-c can multibind to composites, there will
  // be a request sent to the Driver Index to match it to a composite node spec.
  AddCompositeNodeSpec("composite-a", {"node-a", "node-c"});
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-c", 1}});

  AddCompositeNodeSpec_EXPECT_QUEUED("composite-b", {"node-a", "node-c"});

  // Complete the ongoing bind. It should kickstart another bind process.
  DriverIndexReplyWithComposite("node-c", {{"composite-a", 1}});
  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  VerifyCompositeNodeExists(true, "composite-a");
  VerifyMultibindNodes({"node-c"});

  VerifyBindOngoingWithRequests({{"node-c", 1}});
  DriverIndexReplyWithComposite("node-c", {{"composite-a", 0}, {"composite-b", 1}});
  VerifyCompositeNodeExists(false, "composite-b");

  VerifyNoOngoingBind();
}

TEST_F(MultibindTest, MultibindSpecs_NoOverlapBind) {
  AddAndOrphanNode("node-a", /* enable_multibind */ true);
  AddAndOrphanNode("node-b", /* enable_multibind */ true);
  AddAndOrphanNode("node-c", /* enable_multibind */ true);
  AddAndOrphanNode("node-d", /* enable_multibind */ true);
  VerifyNoOngoingBind();

  // Add composite-a. It should trigger bind all available.
  AddCompositeNodeSpec("composite-a", {"node-a", "node-b", "node-d"});
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}, {"node-c", 1}, {"node-d", 1}});

  // Match associated nodes to composite-a.
  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}});
  DriverIndexReplyWithNoMatch("node-c");
  DriverIndexReplyWithComposite("node-d", {{"composite-a", 2}});

  VerifyMultibindNodes({"node-a", "node-b", "node-d"});
  VerifyCompositeNodeExists(true, "composite-a");
  VerifyNoOngoingBind();

  // Add a spec that shares a node with composite-a. It should trigger bind all available.
  AddCompositeNodeSpec("composite-b", {"node-b", "node-c"});
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}, {"node-c", 1}, {"node-d", 1}});

  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}, {"composite-b", 0}});
  DriverIndexReplyWithComposite("node-c", {{"composite-b", 1}});
  DriverIndexReplyWithComposite("node-d", {{"composite-a", 2}});

  VerifyCompositeNodeExists(true, "composite-a");
  VerifyNoOngoingBind();
}

TEST_F(MultibindTest, MultibindSpecs_OverlapBind) {
  AddAndOrphanNode("node-a", /* enable_multibind */ true);
  AddAndOrphanNode("node-b", /* enable_multibind */ true);
  AddAndOrphanNode("node-c", /* enable_multibind */ true);
  AddAndOrphanNode("node-d", /* enable_multibind */ true);
  VerifyNoOngoingBind();

  // Add composite-a. It should trigger bind all available.
  AddCompositeNodeSpec("composite-a", {"node-a", "node-b", "node-d"});
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}, {"node-c", 1}, {"node-d", 1}});

  // Add a new spec while the nodes are matched.
  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}});
  AddCompositeNodeSpec_EXPECT_QUEUED("composite-b", {"node-b", "node-c"});
  DriverIndexReplyWithNoMatch("node-c");
  DriverIndexReplyWithComposite("node-d", {{"composite-a", 2}});

  // We should have a new bind ongoing process.
  VerifyBindOngoingWithRequests({{"node-a", 1}, {"node-b", 1}, {"node-c", 1}, {"node-d", 1}});
  VerifyMultibindNodes({"node-a", "node-b", "node-d"});
  VerifyCompositeNodeExists(true, "composite-a");

  // Match the nodes to the composites.
  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}, {"composite-b", 0}});
  DriverIndexReplyWithNoMatch("node-c");
  DriverIndexReplyWithComposite("node-d", {{"composite-a", 2}, {"composite-b", 1}});

  VerifyMultibindNodes({"node-a", "node-b", "node-d"});
  VerifyCompositeNodeExists(true, "composite-b");
  VerifyNoOngoingBind();
}

TEST_F(MultibindTest, AddSpecsThenNodes) {
  // Add composite-a and composite-b
  AddCompositeNodeSpec("composite-a", {"node-a", "node-b", "node-c"});
  AddCompositeNodeSpec("composite-b", {"node-b", "node-d"});
  VerifyNoOngoingBind();

  AddAndBindNode("node-a", /* enable_multibind */ true);
  VerifyBindOngoingWithRequests({{"node-a", 1}});
  DriverIndexReplyWithComposite("node-a", {{"composite-a", 0}});
  VerifyMultibindNodes({"node-a"});
  VerifyNoOngoingBind();

  AddAndBindNode("node-b", /* enable_multibind */ true);
  VerifyBindOngoingWithRequests({{"node-b", 1}});
  DriverIndexReplyWithComposite("node-b", {{"composite-a", 1}, {"composite-b", 0}});
  VerifyMultibindNodes({"node-a", "node-b"});
  VerifyNoOngoingBind();

  AddAndBindNode("node-c", /* enable_multibind */ false);
  VerifyBindOngoingWithRequests({{"node-c", 1}});
  DriverIndexReplyWithComposite("node-c", {{"composite-a", 2}});
  VerifyMultibindNodes({"node-a", "node-b"});
  VerifyCompositeNodeExists(true, "composite-a");
  VerifyNoOngoingBind();

  AddAndBindNode("node-d", /* enable_multibind */ true);
  VerifyBindOngoingWithRequests({{"node-d", 1}});
  DriverIndexReplyWithComposite("node-d", {{"composite-b", 1}});
  VerifyMultibindNodes({"node-a", "node-b", "node-d"});
  VerifyCompositeNodeExists(true, "composite-b");
  VerifyNoOngoingBind();
}
