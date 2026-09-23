// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/power/all_drivers_element.h"

#include <gtest/gtest.h>

#include "src/devices/bin/driver_manager/power/power_manager.h"
#include "src/devices/bin/driver_manager/tests/driver_manager_test_base.h"

namespace driver_manager {

class FakeTopology : public fidl::Server<fuchsia_power_broker::Topology> {
 public:
  void Lease(LeaseRequest& request, LeaseCompleter::Sync& completer) override {
    lease_requests_.push_back(std::move(request));
    completer.Reply(fit::ok());
  }

  void AddElement(AddElementRequest& request, AddElementCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Topology> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  std::vector<LeaseRequest> lease_requests_;
};

class AllDriversElementTest : public DriverManagerTestBase {
 public:
  NodeManager* GetNodeManager() override { return &node_manager_; }

  static void CompleteStartupTransition(AllDriversElement& element) {
    element.CompleteStartupTransition();
  }

  static void SetCurrentLevel(AllDriversElement& element, uint32_t level) {
    element.current_level_ = level;
  }

  static void RemoveAncestors(AllDriversElement& element, const std::shared_ptr<const Node>& node) {
    element.RemoveAncestors(node);
  }

  static void AddLeafDriverInstance(AllDriversElement& element,
                                    const std::shared_ptr<const Node>& node) {
    element.leaf_driver_instances_.insert_or_assign(node->MakeTopologicalPath(), node);
  }

  static void AddLease(AllDriversElement& element, const std::shared_ptr<const Node>& node,
                       zx::eventpair lease) {
    element.leases_.insert_or_assign(node->MakeTopologicalPath(), std::move(lease));
  }

  static const std::unordered_map<std::string, std::weak_ptr<const Node>>& GetLeafDriverInstances(
      const AllDriversElement& element) {
    return element.leaf_driver_instances_;
  }

  static const std::unordered_map<std::string, zx::eventpair>& GetLeases(
      const AllDriversElement& element) {
    return element.leases_;
  }

 private:
  TestNodeManagerBase node_manager_;
};

}  // namespace driver_manager

namespace driver_manager {
namespace {

// Test CompleteStartupTransition with a Linear Chain topology: root -> child_a -> child_b.
// After transition, only child_b (the leaf) should be a leaf driver instance and hold the lease.
TEST_F(AllDriversElementTest, TestCompleteStartupTransitionLinearChain) {
  auto child_a = CreateNode("child_a", root());
  auto child_b = CreateNode("child_b", child_a);

  child_a->set_bound_for_testing(true);
  child_b->set_bound_for_testing(true);

  zx::eventpair lease_a, lease_a_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_a, &lease_a_peer), ZX_OK);
  child_a->set_startup_lease_for_testing(std::move(lease_a));

  zx::eventpair lease_b, lease_b_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_b, &lease_b_peer), ZX_OK);
  child_b->set_startup_lease_for_testing(std::move(lease_b));

  AllDriversElement element(nullptr, root());
  SetCurrentLevel(element, 1);

  CompleteStartupTransition(element);

  // child_b should be in leaf_driver_instances_ and leases_.
  EXPECT_TRUE(GetLeafDriverInstances(element).contains(child_b->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeafDriverInstances(element).contains(child_a->MakeTopologicalPath()));

  EXPECT_TRUE(GetLeases(element).contains(child_b->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(child_a->MakeTopologicalPath()));

  // Both startup leases should be taken/released.
  auto lease_a_taken = child_a->TakeStartupLease();
  EXPECT_TRUE(!lease_a_taken.has_value() || !lease_a_taken->is_valid());

  auto lease_b_taken = child_b->TakeStartupLease();
  EXPECT_TRUE(!lease_b_taken.has_value() || !lease_b_taken->is_valid());
}

// Test CompleteStartupTransition with a Branching Tree topology: root -> child_a, root -> child_b.
// After transition, both child_a and child_b should be leaf driver instances and hold leases.
TEST_F(AllDriversElementTest, TestCompleteStartupTransitionBranchingTree) {
  auto child_a = CreateNode("child_a", root());
  auto child_b = CreateNode("child_b", root());

  child_a->set_bound_for_testing(true);
  child_b->set_bound_for_testing(true);

  zx::eventpair lease_a, lease_a_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_a, &lease_a_peer), ZX_OK);
  child_a->set_startup_lease_for_testing(std::move(lease_a));

  zx::eventpair lease_b, lease_b_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_b, &lease_b_peer), ZX_OK);
  child_b->set_startup_lease_for_testing(std::move(lease_b));

  AllDriversElement element(nullptr, root());
  SetCurrentLevel(element, 1);

  CompleteStartupTransition(element);

  // Both should be in leaf_driver_instances_ and leases_.
  EXPECT_TRUE(GetLeafDriverInstances(element).contains(child_a->MakeTopologicalPath()));
  EXPECT_TRUE(GetLeafDriverInstances(element).contains(child_b->MakeTopologicalPath()));

  EXPECT_TRUE(GetLeases(element).contains(child_a->MakeTopologicalPath()));
  EXPECT_TRUE(GetLeases(element).contains(child_b->MakeTopologicalPath()));

  // Both startup leases should be taken/released.
  auto lease_a_taken = child_a->TakeStartupLease();
  EXPECT_TRUE(!lease_a_taken.has_value() || !lease_a_taken->is_valid());

  auto lease_b_taken = child_b->TakeStartupLease();
  EXPECT_TRUE(!lease_b_taken.has_value() || !lease_b_taken->is_valid());
}

// Test CompleteStartupTransition with a Diamond DAG topology:
// root -> parent_a -> composite, root -> parent_b -> composite.
// After transition, only the composite node (the leaf) should be a leaf driver instance.
TEST_F(AllDriversElementTest, TestCompleteStartupTransitionDiamondDAG) {
  auto parent_a = CreateNode("parent_a", root());
  auto parent_b = CreateNode("parent_b", root());

  std::vector<std::weak_ptr<Node>> parents = {parent_a, parent_b};
  auto composite = CreateCompositeNode("composite", parents, {{}, {}});

  parent_a->set_bound_for_testing(true);
  parent_b->set_bound_for_testing(true);
  composite->set_bound_for_testing(true);

  zx::eventpair lease_a, lease_a_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_a, &lease_a_peer), ZX_OK);
  parent_a->set_startup_lease_for_testing(std::move(lease_a));

  zx::eventpair lease_b, lease_b_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_b, &lease_b_peer), ZX_OK);
  parent_b->set_startup_lease_for_testing(std::move(lease_b));

  zx::eventpair lease_c, lease_c_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_c, &lease_c_peer), ZX_OK);
  composite->set_startup_lease_for_testing(std::move(lease_c));

  AllDriversElement element(nullptr, root());
  SetCurrentLevel(element, 1);

  CompleteStartupTransition(element);

  // Only composite should be in leaf_driver_instances_ and leases_.
  EXPECT_TRUE(GetLeafDriverInstances(element).contains(composite->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeafDriverInstances(element).contains(parent_a->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeafDriverInstances(element).contains(parent_b->MakeTopologicalPath()));

  EXPECT_TRUE(GetLeases(element).contains(composite->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(parent_a->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(parent_b->MakeTopologicalPath()));

  // All startup leases should be taken/released.
  auto lease_a_taken = parent_a->TakeStartupLease();
  EXPECT_TRUE(!lease_a_taken.has_value() || !lease_a_taken->is_valid());

  auto lease_b_taken = parent_b->TakeStartupLease();
  EXPECT_TRUE(!lease_b_taken.has_value() || !lease_b_taken->is_valid());

  auto lease_c_taken = composite->TakeStartupLease();
  EXPECT_TRUE(!lease_c_taken.has_value() || !lease_c_taken->is_valid());
}

// Test RemoveAncestors on a linear chain: root -> parent -> child.
// When parent is mock-added as leaf, calling RemoveAncestors(child) should remove it.
TEST_F(AllDriversElementTest, TestRemoveAncestorsLinearChain) {
  auto parent = CreateNode("parent", root());
  auto child = CreateNode("child", parent);

  parent->set_bound_for_testing(true);
  child->set_bound_for_testing(true);

  AllDriversElement element(nullptr, root());
  AddLeafDriverInstance(element, parent);

  zx::eventpair lease, lease_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease, &lease_peer), ZX_OK);
  AddLease(element, parent, std::move(lease));

  // RemoveAncestors(child) should clear the parent from leaves.
  RemoveAncestors(element, child);

  EXPECT_FALSE(GetLeafDriverInstances(element).contains(parent->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(parent->MakeTopologicalPath()));
}

// Test RemoveAncestors with a Diamond DAG topology.
// Calling RemoveAncestors(composite) should remove both parent_a and parent_b.
TEST_F(AllDriversElementTest, TestRemoveAncestorsDiamondDAG) {
  auto parent_a = CreateNode("parent_a", root());
  auto parent_b = CreateNode("parent_b", root());

  std::vector<std::weak_ptr<Node>> parents = {parent_a, parent_b};
  auto composite = CreateCompositeNode("composite", parents, {{}, {}});

  parent_a->set_bound_for_testing(true);
  parent_b->set_bound_for_testing(true);
  composite->set_bound_for_testing(true);

  AllDriversElement element(nullptr, root());
  AddLeafDriverInstance(element, parent_a);
  AddLeafDriverInstance(element, parent_b);

  zx::eventpair lease_a, lease_a_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_a, &lease_a_peer), ZX_OK);
  AddLease(element, parent_a, std::move(lease_a));

  zx::eventpair lease_b, lease_b_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease_b, &lease_b_peer), ZX_OK);
  AddLease(element, parent_b, std::move(lease_b));

  RemoveAncestors(element, composite);

  EXPECT_FALSE(GetLeafDriverInstances(element).contains(parent_a->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeafDriverInstances(element).contains(parent_b->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(parent_a->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(parent_b->MakeTopologicalPath()));
}

// Test RemoveAncestors traversing through an unbound parent to reach and remove a bound
// grandparent.
TEST_F(AllDriversElementTest, TestRemoveAncestorsUnboundTraversed) {
  auto grandparent = CreateNode("grandparent", root());
  auto parent = CreateNode("parent", grandparent);
  auto child = CreateNode("child", parent);

  grandparent->set_bound_for_testing(true);
  parent->set_bound_for_testing(false);  // unbound parent!
  child->set_bound_for_testing(true);

  AllDriversElement element(nullptr, root());
  AddLeafDriverInstance(element, grandparent);

  zx::eventpair lease, lease_peer;
  ASSERT_EQ(zx::eventpair::create(0, &lease, &lease_peer), ZX_OK);
  AddLease(element, grandparent, std::move(lease));

  // RemoveAncestors(child) should traverse through unbound parent and find/erase grandparent.
  RemoveAncestors(element, child);

  EXPECT_FALSE(GetLeafDriverInstances(element).contains(grandparent->MakeTopologicalPath()));
  EXPECT_FALSE(GetLeases(element).contains(grandparent->MakeTopologicalPath()));
}

TEST_F(AllDriversElementTest, TestLeaseAllDrivers) {
  auto [topology_client, topology_server] =
      fidl::Endpoints<fuchsia_power_broker::Topology>::Create();

  FakeTopology fake_topology;
  fidl::ServerBindingGroup<fuchsia_power_broker::Topology> bindings;
  bindings.AddBinding(dispatcher(), std::move(topology_server), &fake_topology,
                      fidl::kIgnoreBindingClosure);

  PowerManager power_manager(dispatcher(), std::move(topology_client), std::nullopt, false);

  // Setup a topology: parent_node (unbound) -> child_a (bound), child_b (unbound) -> grandchild
  // (bound)
  auto parent = CreateNode("parent_node");
  auto child_a = CreateNode("child_a", parent);
  child_a->set_bound_for_testing(true);

  auto child_b = CreateNode("child_b", parent);
  child_b->set_bound_for_testing(false);

  auto grandchild = CreateNode("grandchild", child_b);
  grandchild->set_bound_for_testing(true);

  // Call LeaseAllDrivers
  power_manager.LeaseAllDrivers(parent, [] {});

  RunLoopUntilIdle();

  // It should have acquired leases for child_a and grandchild (both are bound nodes in the
  // topology)
  ASSERT_EQ(fake_topology.lease_requests_.size(), 2u);

  // Collect lease names to verify
  std::unordered_set<std::string> leased_names;
  for (const auto& req : fake_topology.lease_requests_) {
    ASSERT_TRUE(req.lease_name().has_value());
    leased_names.insert(*req.lease_name());
  }

  EXPECT_TRUE(leased_names.contains("child_a"));
  EXPECT_TRUE(leased_names.contains("grandchild"));
  EXPECT_FALSE(leased_names.contains("parent_node"));
  EXPECT_FALSE(leased_names.contains("child_b"));
}

}  // namespace
}  // namespace driver_manager
