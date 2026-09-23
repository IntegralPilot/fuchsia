// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/btree.h>
#include <malloc.h>

#include <compare>
#include <numeric>
#include <random>
#include <set>

#include <fbl/ref_counted.h>
#include <zxtest/zxtest.h>

namespace {

// Testing allocator that is backed by the heap and can be controlled to succeed and fail
// allocations in different ways.
struct HeapTestingAllocator {
  ~HeapTestingAllocator() {
    // Ensure no memory leaks. All allocations should have been returned.
    ZX_ASSERT(outstanding_allocs == 0);
  }
  // Allocation interface expected by the btree.
  void* allocate(size_t size_align) {
    switch (behavior) {
      case Behavior::Random:
        if (std::uniform_int_distribution(0, 1)(rng) == 0) {
          return nullptr;
        }
        [[fallthrough]];
      case Behavior::Succeed:
        allow_allocs--;
        if (allow_allocs == 0) {
          behavior = Behavior::Fail;
        }
        outstanding_allocs++;
        return memalign(size_align, size_align);
      case Behavior::Fail:
        return nullptr;
    }
  }
  // Deallocation interface expected by the btree.
  void deallocate(size_t size_align, void* ptr) {
    outstanding_allocs--;
    free(ptr);
  }

  // Cause allocations to be failed based on a fixed pseudo-random pattern.
  void set_random() {
    behavior = Behavior::Random;
    allow_allocs = UINT64_MAX;
  }
  // Fail all allocations.
  void set_fail() { behavior = Behavior::Fail; }
  // Succeed allocations.
  void set_succeed() {
    behavior = Behavior::Succeed;
    allow_allocs = UINT64_MAX;
  }
  // Succeed the specified number of allocations, behavior switching to failing them.
  void set_succeed(size_t count) {
    behavior = Behavior::Succeed;
    allow_allocs = count;
  }
  enum class Behavior {
    Succeed,
    Fail,
    Random,
  };
  Behavior behavior = Behavior::Succeed;
  uint64_t allow_allocs = UINT64_MAX;
  size_t outstanding_allocs = 0;
  std::mt19937_64 rng{0x8a344d45e080e324};
};

// To allow the storage to be elided when an allocator has no state the btree stores an instance of
// an allocator, and not a reference to one, so use this indirection interface to allow us to
// effectively modify the allocator its using.
struct IndirectAllocator {
  void* allocate(size_t size_align) { return alloc.allocate(size_align); }
  void deallocate(size_t size_align, void* ptr) { alloc.deallocate(size_align, ptr); }
  HeapTestingAllocator& alloc;
};

template <typename K, typename V>
using BTree =
    btree::BTree<K, V, btree::NoopObserver, IndirectAllocator, btree::DefaultTypeTraits<V>, 256,
                 btree::IteratorValidation::Tracked, btree::TreeValidation::Assert>;

TEST(BTreeTest, Smoke) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t*> tree(IndirectAllocator{alloc});
  uint64_t item = 42;

  // Initially empty.
  EXPECT_TRUE(tree.is_empty());
  EXPECT_TRUE(tree.begin() == tree.end());

  // Insert a single item.
  {
    auto it = tree.insert(3, &item);
    EXPECT_TRUE(it == tree.find(3));
    auto [key, value] = *it;
    EXPECT_EQ(key, 3u);
    EXPECT_EQ(value, &item);
  }

  // No longer empty.
  EXPECT_FALSE(tree.begin() == tree.end());
  EXPECT_FALSE(tree.is_empty());

  // Single item definitely first item.
  {
    auto [key, value] = *tree.begin();
    EXPECT_EQ(key, 3u);
    EXPECT_EQ(value, &item);
  }

  // Find should not return the wrong item.
  {
    auto it = tree.find(4);
    EXPECT_TRUE(it == tree.end());
    it = tree.find(2);
    EXPECT_TRUE(it == tree.end());
  }

  // Erasing should make the tree empty.
  tree.erase(tree.begin());
  EXPECT_TRUE(tree.begin() == tree.end());

  {
    auto it = tree.find(3);
    EXPECT_TRUE(it == tree.end());
  }
}

struct TestNode : public fbl::RefCounted<TestNode> {
  explicit TestNode(uint64_t v) : value(v) {}
  uint64_t value;
};

struct DestructionTracker {
  explicit DestructionTracker(bool* destroyed) : destroyed_(destroyed) {}
  ~DestructionTracker() { *destroyed_ = true; }
  bool* destroyed_;
};

TEST(BTreeTest, ManagedPointers) {
  HeapTestingAllocator alloc;
  {
    BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});
    auto it = tree.insert(10, 20);
    EXPECT_TRUE(it != tree.end());
    it = tree.begin();
    EXPECT_FALSE(it == tree.end());
    EXPECT_EQ((*it).second, 20u);
    tree.erase(tree.begin());
    EXPECT_TRUE(tree.is_empty());
  }
  {
    BTree<uint64_t, std::unique_ptr<DestructionTracker>> tree(IndirectAllocator{alloc});
    bool destroyed = false;
    std::unique_ptr<DestructionTracker> val = std::make_unique<DestructionTracker>(&destroyed);
    auto it = tree.insert(1, std::move(val));
    EXPECT_TRUE(it != tree.end());
    EXPECT_FALSE(val);
    EXPECT_FALSE(destroyed);
    tree.erase(tree.begin());
    EXPECT_TRUE(tree.is_empty());
    EXPECT_TRUE(destroyed);
  }
  {
    BTree<uint64_t, fbl::RefPtr<TestNode>> tree(IndirectAllocator{alloc});
    fbl::RefPtr<TestNode> node = fbl::MakeRefCounted<TestNode>(42);
    auto it = tree.insert(1, fbl::RefPtr(node));
    EXPECT_TRUE(it != tree.end());
    ASSERT_TRUE(node);
    EXPECT_EQ(node->value, 42);
    node.reset();
    tree.erase(tree.find(1));
    EXPECT_TRUE(tree.is_empty());
  }
}

TEST(BTreeTest, AllocationFailure) {
  HeapTestingAllocator alloc;

  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  // An empty tree must allocate to insert.
  alloc.set_fail();

  auto it = tree.insert(0, 0);
  EXPECT_TRUE(it == tree.end());
  EXPECT_TRUE(tree.is_empty());
  EXPECT_EQ(alloc.outstanding_allocs, 0u);

  // Attempt to construct a completely full 4 level hierarchy where:
  // Level 1: 1 root node, 3 slots
  // Level 2: 3 intermediate nodes, each 15 slots
  // Level 3: 3 * 15 times intermediate nodes, each 15 slots
  // Level 4: 3 * 15 * 15 leaf nodes, each 15 slots
  static constexpr size_t kRootSlots = 3;
  static constexpr size_t kLevel2Nodes = kRootSlots;
  static constexpr size_t kLevel3Nodes = kLevel2Nodes * 15;
  static constexpr size_t kLeafNodes = kLevel3Nodes * 15;
  static constexpr size_t kLeafSlots = kLeafNodes * 15;

  alloc.set_succeed();
  for (size_t i = 0; i < kLeafSlots; i++) {
    it = tree.insert(i, i);
    EXPECT_TRUE(*it == std::make_pair(i, i));
  }

  // Validate allocated nodes are as we expect.
  auto util = tree.calculate_utilization_slow();
  EXPECT_EQ(util.root_size_bytes, 64u);
  EXPECT_EQ(util.stored_values, kLeafSlots);
  EXPECT_EQ(util.num_non_root_nodes, kLevel2Nodes + kLevel3Nodes + kLeafNodes);

  // Attempting to insert again should fail with allocations disallowed.
  alloc.set_fail();
  it = tree.insert(kLeafSlots, kLeafSlots);
  EXPECT_TRUE(it == tree.end());

  // An allocation needs to be done at every level before the insertion can succeed.
  for (size_t i = 1; i < 4; i++) {
    alloc.set_succeed(i);
    it = tree.insert(kLeafSlots, kLeafSlots);
    EXPECT_TRUE(it == tree.end());
  }
  alloc.set_succeed(4);
  it = tree.insert(kLeafSlots, kLeafSlots);
  EXPECT_TRUE(*it == std::make_pair(kLeafSlots, kLeafSlots));
}

// Helper method for performing a test that inserts, finds and then erases in different patterns.
using InsertMethod =
    std::function<BTree<uint64_t, uint64_t*>::iterator(BTree<uint64_t, uint64_t*>&, uint64_t*)>;
using ShuffleMethod = std::function<void(std::vector<size_t>&)>;
void single_insert_find_erase_many_test(size_t key_space, size_t num_keys, InsertMethod& do_insert,
                                        ShuffleMethod& shuffle_insert,
                                        ShuffleMethod& shuffle_erase) {
  ASSERT_TRUE(num_keys <= key_space);

  // Initialize the key space
  std::vector<size_t> items;
  items.resize(key_space);
  std::iota(items.begin(), items.end(), 0);

  // Determine the actual items we will be inserting and their order.
  std::vector<size_t> order = items;
  shuffle_insert(order);
  order.resize(num_keys);

  // Set the allocator to randomly fail. We will just keep retrying any failed insertion until it
  // succeeds.
  HeapTestingAllocator alloc;
  alloc.set_random();
  BTree<uint64_t, uint64_t*> tree(IndirectAllocator{alloc});

  // Keep a parallel set of expected keys to simplify validity checks later on.
  std::set<uint64_t> expected;
  for (auto index : order) {
    auto it = tree.end();
    while (it == tree.end()) {
      it = do_insert(tree, &items[index]);
    }
    EXPECT_TRUE(*it == std::make_pair(items[index], &items[index]));
    EXPECT_TRUE(it == tree.find(items[index]));
    expected.insert(items[index]);
  }

  EXPECT_FALSE(tree.begin() == tree.end());
  // Test forwards iteration.
  {
    auto expected_it = expected.begin();
    for (auto [key, value] : tree) {
      EXPECT_EQ(key, *expected_it);
      EXPECT_EQ(value, &items[*expected_it]);
      expected_it++;
    }
    EXPECT_TRUE(expected_it == expected.end());
  }
  // Test reverse iteration.
  {
    auto expected_it = expected.rbegin();
    for (auto it = tree.end(); it != tree.begin();) {
      it--;
      EXPECT_TRUE(*it == std::make_pair(*expected_it, &items[*expected_it]));
      expected_it++;
    }
  }

  // Ensure that after inserting everything all items are findable.
  for (auto key : expected) {
    auto it = tree.find(key);
    EXPECT_TRUE(it.IsValid());
    EXPECT_TRUE(*it == std::make_pair(key, &items[key]));
  }

  // Create the erase order.
  std::ranges::sort(order);
  shuffle_erase(order);

  for (auto key : order) {
    // Erase from both the tree and the expected set, validating that the returned iterator to the
    // next item is correct.
    auto next = tree.erase(tree.find(key));
    auto next_expected = expected.erase(expected.find(key));
    EXPECT_TRUE((next == tree.end() && next_expected == expected.end()) ||
                ((*next).first == *next_expected));
    EXPECT_TRUE(tree.find(key) == tree.end());
  }
  EXPECT_TRUE(tree.is_empty());
}

TEST(BTreeTest, InsertFindEraseMany) {
  auto rng_shuffle = [](uint64_t seed, std::vector<size_t>& vec) {
    std::mt19937_64 rng;
    rng.seed(seed);
    std::ranges::shuffle(vec, rng);
  };

  // Different insert/erase orderings.
  std::function<void(std::vector<size_t>&)> shuffles[] = {
      [](std::vector<size_t>& vec) {},
      [](std::vector<size_t>& vec) { std::swap(*vec.begin(), *vec.rbegin()); },
      [](std::vector<size_t>& vec) { std::ranges::reverse(vec); },
      [&rng_shuffle](std::vector<size_t>& vec) { rng_shuffle(0x8a344d45e080e324, vec); },
      [&rng_shuffle](std::vector<size_t>& vec) { rng_shuffle(0xadbff1880c9ce89b, vec); },
      [&rng_shuffle](std::vector<size_t>& vec) { rng_shuffle(0x9a068f41344eec43, vec); },
  };

  // Insertion strategies to test insertion hints.
  std::function<BTree<uint64_t, uint64_t*>::iterator(BTree<uint64_t, uint64_t*>&, uint64_t*)>
      inserts[] = {
          [](BTree<uint64_t, uint64_t*>& tree, uint64_t* item) { return tree.insert(*item, item); },
          [](BTree<uint64_t, uint64_t*>& tree, uint64_t* item) {
            return tree.insert(tree.upper_bound(*item), *item, item);
          },
          [](BTree<uint64_t, uint64_t*>& tree, uint64_t* item) {
            auto it = tree.upper_bound(*item);
            it--;
            return tree.insert(it, *item, item);
          },
      };

  // Number of items that place in the tree
  size_t kNumItems[] = {1, 4, 40, 600, 10000};

  // Multiplier applied to the number of items to determine key space. Key space > number of items
  // allows for final tree to not necessarily hold purely consecutive keys.
  size_t kKeyMultiplier[] = {1, 2, 8};

  // Try all permutations.
  for (auto items : kNumItems) {
    for (auto mult : kKeyMultiplier) {
      for (auto& insert : inserts) {
        for (auto& insert_shuffle : shuffles) {
          for (auto& erase_shuffle : shuffles) {
            single_insert_find_erase_many_test(items * mult, items, insert, insert_shuffle,
                                               erase_shuffle);
          }
        }
      }
    }
  }
}

void run_bounds_test(std::vector<uint64_t>& insertion_order) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  // Empty tree.
  EXPECT_TRUE(tree.lower_bound(10) == tree.end());
  EXPECT_TRUE(tree.upper_bound(10) == tree.end());

  for (uint64_t i : insertion_order) {
    auto it = tree.insert(i, i);
    EXPECT_TRUE(it != tree.end());
  }

  // Exact match.
  {
    auto it = tree.lower_bound(20);
    EXPECT_EQ((*it).first, 20u);
    it = tree.upper_bound(20);
    EXPECT_EQ((*it).first, 30u);
  }

  // Before first.
  {
    auto it = tree.lower_bound(5);
    EXPECT_EQ((*it).first, 10u);
    it = tree.upper_bound(5);
    EXPECT_EQ((*it).first, 10u);
  }

  // After last.
  {
    auto it = tree.lower_bound(105);
    EXPECT_TRUE(it == tree.end());
    it = tree.upper_bound(105);
    EXPECT_TRUE(it == tree.end());
  }

  // Between elements.
  {
    auto it = tree.lower_bound(25);
    EXPECT_EQ((*it).first, 30u);
    it = tree.upper_bound(25);
    EXPECT_EQ((*it).first, 30u);
  }

  // At the end.
  {
    auto it = tree.lower_bound(100);
    EXPECT_EQ((*it).first, 100u);
    it = tree.upper_bound(100);
    EXPECT_TRUE(it == tree.end());
  }
}

TEST(BTreeTest, Bounds) {
  // Test with ascending (tail insertion)
  std::vector<uint64_t> ascending = {10, 20, 30, 40, 50, 60, 70, 80, 90, 100};
  run_bounds_test(ascending);

  // Test with descending (head insertion)
  std::vector<uint64_t> descending = {100, 90, 80, 70, 60, 50, 40, 30, 20, 10};
  run_bounds_test(descending);

  // Test with mixed (forces splits, merges, normal rebalancing)
  std::vector<uint64_t> mixed = {50, 10, 80, 20, 90, 30, 100, 40, 70, 60};
  run_bounds_test(mixed);
}

TEST(BTreeTest, ReverseIteration) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  for (uint64_t i = 0; i < 100; i++) {
    auto it = tree.insert(i, i);
    EXPECT_TRUE(it != tree.end());
  }

  auto it = tree.end();
  for (uint64_t i = 100; i > 0; i--) {
    it--;
    EXPECT_EQ((*it).first, i - 1);
  }
  EXPECT_TRUE(it == tree.begin());
}

TEST(BTreeTest, Clear) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  for (uint64_t i = 0; i < 1000; i++) {
    auto it = tree.insert(i, i);
    EXPECT_TRUE(it != tree.end());
  }
  EXPECT_FALSE(tree.is_empty());
  tree.clear();
  EXPECT_TRUE(tree.is_empty());
  EXPECT_TRUE(tree.begin() == tree.end());
  // Clear on an empty tree.
  tree.clear();
  EXPECT_TRUE(tree.is_empty());
}

TEST(BTreeTest, Take) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  auto it = tree.insert(1, 10);
  EXPECT_TRUE(it != tree.end());
  it = tree.insert(2, 20);
  EXPECT_TRUE(it != tree.end());

  auto [key, value] = tree.take(tree.find(1));
  EXPECT_EQ(key, 1u);
  EXPECT_EQ(value, 10u);
  EXPECT_TRUE(tree.find(1) == tree.end());
  EXPECT_EQ(tree.find(2).get().second, 20u);
}

TEST(BTreeTest, EraseEdgeCases) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  // Erase from single element tree.
  auto it = tree.insert(1, 1);
  EXPECT_TRUE(it != tree.end());
  tree.erase(tree.begin());
  EXPECT_TRUE(tree.is_empty());

  // Erase first.
  it = tree.insert(1, 1);
  EXPECT_TRUE(it != tree.end());
  it = tree.insert(2, 2);
  EXPECT_TRUE(it != tree.end());
  tree.erase(tree.begin());
  EXPECT_EQ((*tree.begin()).first, 2u);
  tree.clear();

  // Erase last.
  it = tree.insert(1, 1);
  EXPECT_TRUE(it != tree.end());
  it = tree.insert(2, 2);
  EXPECT_TRUE(it != tree.end());
  it = tree.find(2);
  tree.erase(it);
  EXPECT_EQ((*tree.begin()).first, 1u);
  EXPECT_TRUE(++tree.begin() == tree.end());
}

TEST(BTreeTest, HintedInsertion) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  // Tail insertion hint.
  auto it = tree.insert(1, 1);
  EXPECT_TRUE(it != tree.end());
  it = tree.insert(it, 2, 2);
  EXPECT_TRUE(it != tree.end());
  EXPECT_EQ(tree.find(2).get().second, 2u);

  // Head insertion hint.
  it = tree.find(1);
  it = tree.insert(it, 0, 0);
  EXPECT_TRUE(it != tree.end());
  EXPECT_EQ(tree.find(0).get().second, 0u);

  // Middle insertion hint.
  tree.clear();
  it = tree.insert(1, 1);
  EXPECT_TRUE(it != tree.end());
  it = tree.insert(3, 3);
  EXPECT_TRUE(it != tree.end());
  it = tree.find(3);
  it = tree.insert(it, 2, 2);
  EXPECT_TRUE(it != tree.end());
  EXPECT_EQ(tree.find(2).get().second, 2u);
}

TEST(BTreeTest, Utilization) {
  HeapTestingAllocator alloc;
  BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});

  // Empty tree utilization.
  auto util = tree.calculate_utilization_slow();
  EXPECT_EQ(util.root_size_bytes, 0u);
  EXPECT_EQ(util.num_non_root_nodes, 0u);
  EXPECT_EQ(util.stored_values, 0u);
  EXPECT_EQ(util.nodes_in_bytes(), 0u);

  // Insert a few items to get a size class 0 root.
  auto it1 = tree.insert(1, 1);
  EXPECT_TRUE(it1 != tree.end());
  util = tree.calculate_utilization_slow();
  EXPECT_EQ(util.root_size_bytes, 32u);
  EXPECT_EQ(util.num_non_root_nodes, 0u);
  EXPECT_EQ(util.stored_values, 1u);
  EXPECT_EQ(util.nodes_in_bytes(), 32u);

  // Insert to push it to class 1 (64 bytes -> max count 3).
  auto it2 = tree.insert(2, 2);
  EXPECT_TRUE(it2 != tree.end());
  util = tree.calculate_utilization_slow();
  EXPECT_EQ(util.root_size_bytes, 64u);
  EXPECT_EQ(util.num_non_root_nodes, 0u);
  EXPECT_EQ(util.stored_values, 2u);
  EXPECT_EQ(util.nodes_in_bytes(), 64u);

  auto it3 = tree.insert(3, 3);
  EXPECT_TRUE(it3 != tree.end());
  util = tree.calculate_utilization_slow();
  EXPECT_EQ(util.root_size_bytes, 64u);
  EXPECT_EQ(util.num_non_root_nodes, 0u);
  EXPECT_EQ(util.stored_values, 3u);
  EXPECT_EQ(util.nodes_in_bytes(), 64u);

  // Push to multiple nodes.
  for (uint64_t i = 4; i <= 20; i++) {
    auto it = tree.insert(i, i);
    EXPECT_TRUE(it != tree.end());
  }
  util = tree.calculate_utilization_slow();
  // It should now have multiple nodes.
  EXPECT_GT(util.num_non_root_nodes, 0u);
  EXPECT_EQ(util.stored_values, 20u);

  // Calculate expected bytes.
  uint64_t expected_bytes = util.root_size_bytes;
  expected_bytes += util.num_non_root_nodes * 256u;

  EXPECT_EQ(util.nodes_in_bytes(), expected_bytes);
  // Erase all items to test emptiness again.
  for (uint64_t i = 1; i <= 20; i++) {
    tree.erase(tree.find(i));
  }
  util = tree.calculate_utilization_slow();
  EXPECT_EQ(util.root_size_bytes, 0u);
  EXPECT_EQ(util.num_non_root_nodes, 0u);
  EXPECT_EQ(util.stored_values, 0u);
  EXPECT_EQ(util.nodes_in_bytes(), 0u);
}

TEST(BTreeTest, IntegralTypes) {
  HeapTestingAllocator alloc;
  {
    BTree<uint64_t, int32_t> tree(IndirectAllocator{alloc});
    auto it = tree.insert(1, -42);
    EXPECT_TRUE(it != tree.end());
    int32_t value = (*it).second;
    EXPECT_EQ(value, -42);
    tree.erase(it);
  }
  {
    BTree<uint64_t, uint8_t> tree(IndirectAllocator{alloc});
    auto it = tree.insert(1, 255);
    EXPECT_TRUE(it != tree.end());
    uint8_t value = (*it).second;
    EXPECT_EQ(value, 255);
    tree.erase(it);
  }
  {
    BTree<uint64_t, bool> tree(IndirectAllocator{alloc});
    auto it = tree.insert(1, true);
    EXPECT_TRUE(it != tree.end());
    bool value = (*it).second;
    EXPECT_EQ(value, true);
    tree.erase(it);
  }
}

TEST(BTreeTest, Update) {
  HeapTestingAllocator alloc;
  {
    BTree<uint64_t, uint64_t> tree(IndirectAllocator{alloc});
    auto it = tree.insert(10, 20);
    EXPECT_TRUE(it != tree.end());
    tree.update(it, 30);
    EXPECT_EQ((*it).second, 30u);
    // Iterator is still valid, can update twice.
    tree.update(it, 40);
    EXPECT_EQ((*it).second, 40u);

    it = tree.find(10);
    EXPECT_TRUE(it != tree.end());
    EXPECT_EQ((*it).second, 40u);
  }
  {
    BTree<uint64_t, std::unique_ptr<DestructionTracker>> tree(IndirectAllocator{alloc});
    bool destroyed1 = false;
    bool destroyed2 = false;

    auto val1 = std::make_unique<DestructionTracker>(&destroyed1);
    auto it = tree.insert(1, std::move(val1));
    EXPECT_TRUE(it != tree.end());
    EXPECT_FALSE(destroyed1);

    auto val2 = std::make_unique<DestructionTracker>(&destroyed2);
    tree.update(it, std::move(val2));

    // The first value should be destroyed upon being overwritten.
    EXPECT_TRUE(destroyed1);
    EXPECT_FALSE(destroyed2);

    tree.erase(it);

    // The second value should be destroyed upon erasure.
    EXPECT_TRUE(destroyed2);
    EXPECT_TRUE(tree.is_empty());
  }
}
struct HashObserver {
  struct AugmentedState {
    uint64_t val1 = 0;
    uint64_t val2 = 0;
    bool operator==(const AugmentedState& other) const = default;
  };

  template <typename iterator>
  static AugmentedState UpdateForInsert(AugmentedState old, iterator node_start, iterator node_end,
                                        iterator insert_start, iterator insert_end) {
    return Calculate(node_start, node_end);
  }
  template <typename iterator>
  static AugmentedState Calculate(iterator node_start, iterator node_end) {
    node_end++;
    uint64_t hash = 0x1234567890abcdef;
    for (iterator it = node_start; it != node_end; it++) {
      auto [k, v] = *it;
      hash ^= k;
      hash = (hash << 13) | (hash >> 51);
      hash += static_cast<uint64_t>(v);
    }
    return AugmentedState{.val1 = hash, .val2 = ~hash};
  }
  static AugmentedState Fold(AugmentedState left, AugmentedState right) {
    uint64_t hash = left.val1 ^ right.val1;
    uint64_t res = (hash << 17) | (hash >> 47);
    return AugmentedState{.val1 = res, .val2 = ~res};
  }
};

using HashTree = btree::BTree<uint64_t, uint64_t, HashObserver, IndirectAllocator,
                              btree::DefaultTypeTraits<uint64_t>, 256,
                              btree::IteratorValidation::Tracked, btree::TreeValidation::Assert>;

TEST(BTreeTest, AugmentedHash) {
  HeapTestingAllocator alloc;
  HashTree tree(IndirectAllocator{alloc});

  ASSERT_TRUE(tree.debug_validate_tree());

  // Insert ascending with end() hint (triggers hinted insertion)
  for (uint64_t i = 1; i <= 200; i++) {
    auto it_ins = tree.insert(tree.end(), i * 10, i * 5);
    ASSERT_TRUE(it_ins != tree.end());
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  // Update elements
  for (uint64_t i = 1; i <= 200; i += 3) {
    tree.update(tree.find(i * 10), i * 7);
    ASSERT_TRUE(tree.debug_validate_tree());
    HashObserver::AugmentedState walk_state;
    bool found = false;
    zx_status_t status = tree.walk([](const auto& state) { return ZX_OK; },
                                   [&](const auto& state, auto begin, auto end) {
                                     if (!found) {
                                       for (auto it = begin;; it++) {
                                         if ((*it).first == i * 10) {
                                           walk_state = state;
                                           found = true;
                                           return ZX_ERR_STOP;
                                         }
                                         if (it == end)
                                           break;
                                       }
                                     }
                                     return ZX_OK;
                                   });
    ASSERT_EQ(status, ZX_OK);
    ASSERT_TRUE(found);
  }

  // Insert descending with begin() hint
  for (uint64_t i = 2000; i >= 1800; i--) {
    auto it_ins = tree.insert(tree.begin(), i * 10, i * 5);
    ASSERT_TRUE(it_ins != tree.end());
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  // Insert in-place with hint
  for (uint64_t i = 1; i <= 50; i++) {
    // 5 is between 0 and 10, so hinting find(10) tests in-place hint insertion.
    auto hint_it = tree.find(i * 10);
    auto it_ins = tree.insert(hint_it, i * 10 - 5, i * 2);
    ASSERT_TRUE(it_ins != tree.end());
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  // Erase front
  for (uint64_t i = 1; i <= 50; i++) {
    tree.erase(tree.find(i * 10 - 5));
    ASSERT_TRUE(tree.debug_validate_tree());
    tree.erase(tree.find(i * 10));
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  // Erase back
  for (uint64_t i = 2000; i >= 1950; i--) {
    tree.erase(tree.find(i * 10));
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  // Erase middle
  for (uint64_t i = 100; i <= 150; i++) {
    tree.erase(tree.find(i * 10));
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  tree.clear();
  ASSERT_TRUE(tree.debug_validate_tree());

  // Insert random
  std::mt19937 gen(12345);
  std::vector<uint64_t> keys;
  for (uint64_t i = 1; i <= 300; i++)
    keys.push_back(i * 10);
  std::shuffle(keys.begin(), keys.end(), gen);

  for (uint64_t k : keys) {
    auto it_ins = tree.insert(k, k * 2);
    ASSERT_TRUE(it_ins != tree.end());
    ASSERT_TRUE(tree.debug_validate_tree());
  }

  std::shuffle(keys.begin(), keys.end(), gen);
  for (uint64_t k : keys) {
    tree.erase(tree.find(k));
    ASSERT_TRUE(tree.debug_validate_tree());
  }
}

struct SumObserver {
  struct AugmentedState {
    uint64_t val1 = 0;
    uint64_t val2 = 0;
    bool operator==(const AugmentedState& other) const = default;
  };

  template <typename iterator>
  static AugmentedState Calculate(iterator node_start, iterator node_end) {
    uint64_t sum = 0;
    for (iterator it = node_start;; it++) {
      sum += (*it).first;
      if (it == node_end)
        break;
    }
    return AugmentedState{.val1 = sum, .val2 = 0};
  }
  static AugmentedState Fold(AugmentedState left, AugmentedState right) {
    return AugmentedState{.val1 = left.val1 + right.val1, .val2 = 0};
  }
};

using SumTree = btree::BTree<uint64_t, uint64_t, SumObserver, IndirectAllocator,
                             btree::DefaultTypeTraits<uint64_t>, 256,
                             btree::IteratorValidation::Tracked, btree::TreeValidation::Assert>;

TEST(BTreeTest, Walk) {
  HeapTestingAllocator alloc;
  SumTree tree(IndirectAllocator{alloc});

  // Empty tree walk
  zx_status_t status = tree.walk([](const auto& state) { return ZX_OK; },
                                 [](const auto& state, auto begin, auto end) { return ZX_OK; });
  EXPECT_EQ(status, ZX_OK);

  // Insert 1 to 100 to ensure we have a multi-level tree
  uint64_t total_expected_sum = 0;
  for (uint64_t i = 1; i <= 100; i++) {
    auto it = tree.insert(i, i * 2);
    EXPECT_TRUE(it.IsValid());
    total_expected_sum += i;
  }

  // Walk over all items and compute sum manually to verify full traversal
  uint64_t walked_sum = 0;
  status = tree.walk([](const auto& state) { return ZX_OK; },
                     [&](const auto& state, auto begin, auto end) {
                       for (auto it = begin;; it++) {
                         walked_sum += (*it).first;
                         if (it == end)
                           break;
                       }
                       return ZX_OK;
                     });
  EXPECT_EQ(status, ZX_OK);
  EXPECT_EQ(walked_sum, total_expected_sum);

  // Use ZX_ERR_NEXT to skip subtrees
  uint64_t pruned_sum = 0;
  status = tree.walk(
      [&](const auto& state) {
        // Skip any subtrees where the sum of keys > 200
        if (state.val1 > 200) {
          return ZX_ERR_NEXT;
        }
        return ZX_OK;
      },
      [&](const auto& state, auto begin, auto end) {
        for (auto it = begin;; it++) {
          pruned_sum += (*it).first;
          if (it == end)
            break;
        }
        return ZX_OK;
      });
  EXPECT_EQ(status, ZX_OK);
  EXPECT_LT(pruned_sum, total_expected_sum);

  // Use ZX_ERR_STOP in leaf to stop early
  uint64_t partial_sum = 0;
  status = tree.walk([](const auto& state) { return ZX_OK; },
                     [&](const auto& state, auto begin, auto end) {
                       for (auto it = begin;; it++) {
                         partial_sum += (*it).first;
                         if ((*it).first == 50) {
                           return ZX_ERR_STOP;
                         }
                         if (it == end)
                           break;
                       }
                       return ZX_OK;
                     });
  EXPECT_EQ(status, ZX_OK);
  uint64_t expected_partial = 0;
  for (uint64_t i = 1; i <= 50; i++) {
    expected_partial += i;
  }
  EXPECT_EQ(partial_sum, expected_partial);

  // Use ZX_ERR_STOP in subtree to stop early before exploring
  uint64_t early_subtree_sum = 0;
  status = tree.walk([&](const auto& state) { return ZX_ERR_STOP; },
                     [&](const auto& state, auto begin, auto end) {
                       for (auto it = begin;; it++) {
                         early_subtree_sum += (*it).first;
                         if (it == end)
                           break;
                       }
                       return ZX_OK;
                     });
  EXPECT_EQ(status, ZX_OK);
  EXPECT_EQ(early_subtree_sum, 0u);

  // Any other error status should abort and be returned
  status = tree.walk([](const auto& state) { return ZX_ERR_INTERNAL; },
                     [](const auto& state, auto begin, auto end) { return ZX_OK; });
  EXPECT_EQ(status, ZX_ERR_INTERNAL);
}

struct CustomSmallKey {
  uint32_t part1;
  uint16_t part2;
  uint16_t part3;

  auto operator<=>(const CustomSmallKey& other) const = default;
};

TEST(BTreeTest, CustomSmallKey) {
  HeapTestingAllocator alloc;
  BTree<CustomSmallKey, uint64_t> tree(IndirectAllocator{alloc});

  for (uint32_t i = 1; i <= 100; i++) {
    auto it = tree.insert({i, 0, 0}, i * 2);
    EXPECT_TRUE(it != tree.end());
  }

  auto it = tree.find({50, 0, 0});
  EXPECT_TRUE(it != tree.end());
  EXPECT_EQ((*it).second, 100u);
  EXPECT_TRUE(tree.debug_validate_tree());

  tree.erase(it);
  EXPECT_TRUE(tree.find({50, 0, 0}) == tree.end());
}

struct CustomMediumKey {
  uint64_t part1;
  uint64_t part2;

  auto operator<=>(const CustomMediumKey& other) const = default;
};

TEST(BTreeTest, CustomMediumKey) {
  HeapTestingAllocator alloc;
  BTree<CustomMediumKey, uint64_t> tree(IndirectAllocator{alloc});

  for (uint64_t i = 1; i <= 100; i++) {
    auto it = tree.insert({i, i * 2}, i);
    EXPECT_TRUE(it != tree.end());
  }

  auto it = tree.find({50, 100});
  EXPECT_TRUE(it != tree.end());
  EXPECT_EQ((*it).second, 50u);
  EXPECT_TRUE(tree.debug_validate_tree());

  tree.erase(it);
  EXPECT_TRUE(tree.find({50, 100}) == tree.end());
}

struct CustomLargeKey {
  uint64_t parts[8];

  auto operator<=>(const CustomLargeKey& other) const = default;
};

TEST(BTreeTest, CustomLargeKey) {
  HeapTestingAllocator alloc;
  // NodeSize must be able to hold at least a few items. A 256 byte node can hold ~2-3 72-byte items
  // which is too few so we must override the node size.
  btree::BTree<CustomLargeKey, uint64_t, btree::NoopObserver, IndirectAllocator,
               btree::DefaultTypeTraits<uint64_t>, 1024, btree::IteratorValidation::Tracked,
               btree::TreeValidation::Assert>
      tree(IndirectAllocator{alloc});

  for (uint64_t i = 1; i <= 50; i++) {
    CustomLargeKey key{};
    key.parts[0] = i;
    key.parts[7] = i * 2;
    auto it = tree.insert(key, i);
    EXPECT_TRUE(it != tree.end());
  }

  CustomLargeKey search_key{};
  search_key.parts[0] = 25;
  search_key.parts[7] = 50;
  auto it = tree.find(search_key);
  EXPECT_TRUE(it != tree.end());
  EXPECT_EQ((*it).second, 25u);
  EXPECT_TRUE(tree.debug_validate_tree());

  tree.erase(it);
  EXPECT_TRUE(tree.find(search_key) == tree.end());
}

}  // namespace
