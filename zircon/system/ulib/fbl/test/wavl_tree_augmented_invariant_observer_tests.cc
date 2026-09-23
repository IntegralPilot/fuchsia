// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>
#include <array>
#include <limits>
#include <random>

#include <fbl/intrusive_wavl_tree.h>
#include <fbl/wavl_tree_augmented_invariant_observer.h>
#include <zxtest/zxtest.h>

namespace {

// The definition of the node we will use during testing.
struct TestNode {
  TestNode() = default;
  ~TestNode() { ZX_ASSERT(!subtree_invariants.is_valid()); }

  static constexpr uint32_t kMinValue = std::numeric_limits<uint32_t>::min();
  static constexpr uint32_t kMaxValue = std::numeric_limits<uint32_t>::max();

  struct SubtreeInvariants {
    constexpr bool is_valid() const { return min_value <= max_value; }
    constexpr void reset() {
      min_value = kMaxValue;
      max_value = kMinValue;
    }

    uint32_t min_value{kMaxValue};
    uint32_t max_value{kMinValue};
  };

  uint32_t key{0};
  uint32_t value{0};
  SubtreeInvariants subtree_invariants{};
  fbl::WAVLTreeNodeState<TestNode*> tree_node;
};

// Trait used to locate the WAVL tree node state in TestNode, as well as to establish the sorting
// invariant.
struct TestNodeTraits {
  static uint32_t GetKey(const TestNode& test_node) { return test_node.key; }
  static bool LessThan(uint32_t a, uint32_t b) { return a < b; }
  static bool EqualTo(uint32_t a, uint32_t b) { return a == b; }
  static auto& node_state(TestNode& test_node) { return test_node.tree_node; }
};

// Trait use to maintain the subtree min/max augmented invariants of TestNode::value.
struct AugmentedNodeTraits {
  static TestNode::SubtreeInvariants GetNodeValue(const TestNode& node) {
    return {.min_value = node.value, .max_value = node.value};
  }
  static TestNode::SubtreeInvariants GetSubtreeValue(const TestNode& node) {
    return node.subtree_invariants;
  }
  static TestNode::SubtreeInvariants CombineValues(TestNode::SubtreeInvariants a,
                                                   TestNode::SubtreeInvariants b) {
    return {.min_value = std::min(a.min_value, b.min_value),
            .max_value = std::max(a.max_value, b.max_value)};
  }
  static void SetSubtreeValue(TestNode& node, TestNode::SubtreeInvariants subtree_invariants) {
    node.subtree_invariants = subtree_invariants;
  }
  static void ResetSubtreeValue(TestNode& node) { node.subtree_invariants.reset(); }
};

using TestTree = fbl::WAVLTree<uint32_t, TestNode*, TestNodeTraits, fbl::DefaultObjectTag,
                               fbl::SizeOrder::Constant, TestNodeTraits,

                               fbl::WAVLTreeAugmentedInvariantObserver<AugmentedNodeTraits>>;

template <typename NodeCollection>
void ValidateTree(const TestTree& tree, const NodeCollection& nodes,
                  const TestNode* extra_node = nullptr) {
  uint32_t min_value = TestNode::kMaxValue;
  uint32_t max_value = TestNode::kMinValue;

  auto update_invariants = [&min_value, &max_value](const TestNode& node) {
    if (node.tree_node.InContainer()) {
      ASSERT_TRUE(node.subtree_invariants.is_valid());
      min_value = std::min(min_value, node.value);
      max_value = std::max(max_value, node.value);
    } else {
      ASSERT_FALSE(node.subtree_invariants.is_valid());
    }
  };

  for (const auto& node : nodes) {
    ASSERT_NO_FATAL_FAILURE(update_invariants(node));
  }

  if (extra_node) {
    ASSERT_NO_FATAL_FAILURE(update_invariants(*extra_node));
  }

  if (min_value != TestNode::kMaxValue && max_value != TestNode::kMinValue) {
    ASSERT_FALSE(tree.is_empty());
    ASSERT_EQ(min_value, tree.root()->subtree_invariants.min_value);
    ASSERT_EQ(max_value, tree.root()->subtree_invariants.max_value);
  }

  for (auto iter = tree.begin(); iter != tree.end(); ++iter) {
    uint32_t expected_min = iter->value;
    uint32_t expected_max = iter->value;
    if (iter.left()) {
      expected_min = std::min(expected_min, iter.left()->subtree_invariants.min_value);
      expected_max = std::max(expected_max, iter.left()->subtree_invariants.max_value);
    }
    if (iter.right()) {
      expected_min = std::min(expected_min, iter.right()->subtree_invariants.min_value);
      expected_max = std::max(expected_max, iter.right()->subtree_invariants.max_value);
    }

    ASSERT_EQ(expected_min, iter->subtree_invariants.min_value);
    ASSERT_EQ(expected_max, iter->subtree_invariants.max_value);
  }
}

TEST(WAVLTreeAugmentedInvariantObserverTests, AugmentedInvariaintMaintained) {
  struct TestConfig {
    const uint64_t seed;
    const bool use_clear;
  };

  // Run the test a few different times with different random seeds, and at least once where we
  // clear the entire tree using |clear|, instead of removing the elements one at a time.
  constexpr std::array kConfigs = {
      TestConfig{0x8a344d45e080e324, false},
      TestConfig{0xadbff1880c9ce89b, false},
      TestConfig{0x9a068f41344eec43, true},
  };

  for (const auto& cfg : kConfigs) {
    constexpr uint32_t kTestCount = 256;
    std::array<TestNode, kTestCount> test_nodes;
    std::array<uint32_t, kTestCount> shuffle_order;
    TestTree tree;

    // Initialize our array of TestNodes with unique primary keys, and random augmented values.
    // Also initialize the shuffle order with a set of ascending indices.
    std::mt19937_64 rng(cfg.seed);
    std::uniform_int_distribution<uint32_t> augmented_value_distribution(TestNode::kMinValue + 1,
                                                                         TestNode::kMaxValue);

    for (uint32_t i = 0; i < kTestCount; ++i) {
      test_nodes[i].key = i;
      test_nodes[i].value = augmented_value_distribution(rng);
      shuffle_order[i] = i;
    }

    // Shuffle the order deck and add the test nodes to the tree in the shuffled order, verifying
    // the tree each time.
    ASSERT_NO_FATAL_FAILURE(ValidateTree(tree, test_nodes));
    std::shuffle(shuffle_order.begin(), shuffle_order.end(), rng);
    for (uint32_t ndx : shuffle_order) {
      tree.insert(&test_nodes[ndx]);
      ASSERT_NO_FATAL_FAILURE(ValidateTree(tree, test_nodes));
    }

    // Create a test node which is guaranteed to collide with test_nodes[0].  Also, give it an
    // augmented value which is less then any of the values in the tree.
    TestNode collision_node;
    collision_node.key = 0;
    collision_node.value = TestNode::kMinValue;

    // Attempt an insert-or-find operation using the collision node.  The insert should fail,
    // leaving the currently computed min value unchanged, but if the Traits used included an,
    // OnInsertCollision method, it should have been called.
    typename TestTree::iterator already_in_tree;
    ASSERT_FALSE(tree.insert_or_find(&collision_node, &already_in_tree));
    ASSERT_EQ(&test_nodes[0], &*already_in_tree);
    ASSERT_NO_FATAL_FAILURE(ValidateTree(tree, test_nodes, &collision_node));

    // Now attempt an insert-or-replace using the collision node.  test_nodes[0] should end up being
    // replaced by collision_node, and collision_node.value should become the new min of the tree.
    TestNode* replaced_node = tree.insert_or_replace(&collision_node);
    ASSERT_EQ(&test_nodes[0], replaced_node);
    ASSERT_NO_FATAL_FAILURE(ValidateTree(tree, test_nodes, &collision_node));

    // Depending on the test configuration, either simply clear the tree, or shuffle the deck again
    // and remove the nodes from the tree in the new random order.
    if (cfg.use_clear) {
      tree.clear();
      ASSERT_NO_FATAL_FAILURE(ValidateTree(tree, test_nodes, &collision_node));
    } else {
      std::shuffle(shuffle_order.begin(), shuffle_order.end(), rng);
      for (uint32_t ndx : shuffle_order) {
        // Handle the fact that test_nodes[0] was replaced by collision_node
        if (ndx == 0) {
          tree.erase(collision_node);
        } else {
          tree.erase(test_nodes[ndx]);
        }
        ASSERT_NO_FATAL_FAILURE(ValidateTree(tree, test_nodes, &collision_node));
      }
    }
  }
}

}  // namespace
