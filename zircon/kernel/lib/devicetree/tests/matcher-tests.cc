// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/devicetree/testing/loaded-dtb.h>
#include <lib/fit/function.h>

#include <optional>
#include <string>
#include <string_view>

#include <zxtest/zxtest.h>

namespace {

using devicetree::testing::LoadedDtb;

// Invalid matcher detection by traits.

// Doesn't implement any of the Matcher contract, all traits should be false.
struct InvalidMatcher {};
static_assert(!devicetree::Matcher<InvalidMatcher>);

struct ValidMatcher {
  static constexpr size_t kMaxScans = 1;
  devicetree::ScanState OnNode(const devicetree::NodePath&,
                               const devicetree::PropertyDecoder& decoder);
  void OnError(std::string_view v);
  devicetree::ScanState OnSubtree(const devicetree::NodePath&);
  devicetree::ScanState OnScan();
  void OnDone();
};
static_assert(devicetree::Matcher<ValidMatcher>);

// Helper matcher for tests.
template <size_t ScanBeforeCompletion>
struct SingleNodeMatcher {
  static constexpr size_t kMaxScans = ScanBeforeCompletion;

  template <typename T, typename U, typename V>
  SingleNodeMatcher(std::string_view path_to_match, T&& node_cb, U&& walk_cb, V&& on_subtree_cb)
      : path_to_match(path_to_match), node(node_cb), walk(walk_cb), on_subtree(on_subtree_cb) {}

  template <typename T, typename U>
  SingleNodeMatcher(std::string_view path_to_match, T&& node_cb, U&& walk_cb)
      : SingleNodeMatcher(path_to_match, node_cb, walk_cb, [](const devicetree::NodePath&) {
          return devicetree::ScanState::kActive;
        }) {}

  template <typename T>
  SingleNodeMatcher(std::string_view path_to_match, T&& node_cb)
      : SingleNodeMatcher(path_to_match, node_cb, []() {}) {}

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder) {
    visit_count++;
    auto resolved_path = decoder.ResolvePath(path_to_match);
    if (resolved_path.is_error()) {
      return resolved_path.error_value() ==
                     devicetree::PropertyDecoder::PathResolveError::kNoAliases
                 ? devicetree::ScanState::kNeedsPathResolution
                 : devicetree::ScanState::kDoneWithSubtree;
    }
    switch (path.CompareWith(*resolved_path)) {
      case devicetree::NodePath::Comparison::kEqual:
        found = true;
        node(path.back(), decoder);
        return node_match_result;
      case devicetree::NodePath::Comparison::kParent:
      case devicetree::NodePath::Comparison::kIndirectAncestor:
        return devicetree::ScanState::kActive;
      case devicetree::NodePath::Comparison::kMismatch:
      case devicetree::NodePath::Comparison::kChild:
      case devicetree::NodePath::Comparison::kIndirectDescendent:
        return devicetree::ScanState::kDoneWithSubtree;
    };
  }

  devicetree::ScanState OnSubtree(const devicetree::NodePath& path) {
    on_subtree_count++;
    return on_subtree(path);
  }

  devicetree::ScanState OnScan() {
    walk_count++;
    walk();
    return walk_result;
  }

  void OnDone() { on_done = true; }

  void OnError(std::string_view error) { this->error = error; }

  std::string error;
  std::string_view path_to_match;
  devicetree::ScanState node_match_result = devicetree::ScanState::kDone;
  devicetree::ScanState walk_result = devicetree::ScanState::kDone;
  bool found = false;
  bool on_done = false;
  int visit_count = 0;
  size_t walk_count = 0;
  size_t on_subtree_count = 0;
  fit::function<void(std::string_view, const devicetree::PropertyDecoder&)> node;
  fit::function<void()> walk;
  fit::function<devicetree::ScanState(const devicetree::NodePath&)> on_subtree;
};

class MatchTest : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    auto loaded_dtb = devicetree::testing::LoadDtb("complex_no_properties.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    fdt_no_props_ = *loaded_dtb;

    loaded_dtb = devicetree::testing::LoadDtb("complex_with_alias.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    fdt_no_props_with_alias_ = *loaded_dtb;
  }

  static void TearDownTestuite() { fdt_no_props_ = std::nullopt; }

  /*
         *
        / \
       A   E
      / \   \
     B   C   F
        /   / \
       D   G   I
          /
         H
  */
  devicetree::Devicetree no_prop_tree() { return fdt_no_props_->fdt(); }

  /*
    Same as |no_prop_tree| but with the following aliases:
      * foo = "/A/C"
      * bar = "/E/F"
    In this tree, the aliases node is the last one of the root's offspring.
  */
  devicetree::Devicetree no_prop_tree_with_alias() { return fdt_no_props_with_alias_->fdt(); }

 private:
  static std::optional<devicetree::testing::LoadedDtb> fdt_no_props_;
  static std::optional<devicetree::testing::LoadedDtb> fdt_no_props_with_alias_;
};

std::optional<devicetree::testing::LoadedDtb> MatchTest::fdt_no_props_ = {};
std::optional<devicetree::testing::LoadedDtb> MatchTest::fdt_no_props_with_alias_ = {};

TEST_F(MatchTest, EarlyCompletion) {
  size_t seen = 0;
  SingleNodeMatcher<2> matcher("/A/C/D", [&](auto name, const auto& decoder) {
    seen++;
    EXPECT_EQ(name, "D");
  });

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher));

  // This matcher completes on the first iteration, so the walk count will be 0.
  EXPECT_TRUE(matcher.found);
  EXPECT_EQ(matcher.visit_count, 5);
  EXPECT_EQ(matcher.walk_count, 0);
  EXPECT_TRUE(matcher.error.empty());
  EXPECT_EQ(seen, 1);
  EXPECT_TRUE(matcher.on_done);
}

TEST_F(MatchTest, NoShortCircuitingAliasesNode) {
  // Verify that when no matchers can make progress due to PathResolution being needed,
  // the aliases eventually get resolved, and further progress can be made.
  size_t seen = 0;
  SingleNodeMatcher<1> matcher("foo/D", [&](auto name, const auto& decoder) {
    seen++;
    EXPECT_EQ(name, "D");
    matcher.node_match_result = devicetree::ScanState::kDone;
  });

  auto tree = no_prop_tree_with_alias();
  EXPECT_TRUE(devicetree::Match(tree, matcher));

  // This matcher completes on the second iteration, so the walk count will be 1.
  EXPECT_TRUE(matcher.found);
  // Walk 0: * -> Meeds Path resolution at root. 1 visit.
  // Walk 1: * -> A -> B -> C -> D (Done)
  EXPECT_EQ(matcher.visit_count, 6);

  // Its never called because of DFS, the matcher gets Done.
  EXPECT_EQ(matcher.on_subtree_count, 0);

  EXPECT_EQ(matcher.walk_count, 0);
  EXPECT_TRUE(matcher.error.empty());
  EXPECT_EQ(seen, 1);
  EXPECT_TRUE(matcher.on_done);
}

TEST_F(MatchTest, MultipleWalksForCompletion) {
  size_t seen = 0;
  SingleNodeMatcher<2> matcher("/A/C/D", [&](auto name, const auto& decoder) {
    seen++;
    EXPECT_EQ(name, "D");
    matcher.node_match_result =
        (seen > 1) ? devicetree::ScanState::kDone : devicetree::ScanState::kActive;
  });
  matcher.walk_result = devicetree::ScanState::kActive;

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher));

  // This matcher completes on the second iteration, so the walk count will be 1.
  EXPECT_TRUE(matcher.found);
  // Walk 0: * -> A -> B -> C -> D -> E
  // Walk 1: * -> A -> B -> C -> D (Done)
  EXPECT_EQ(matcher.visit_count, 11);

  // D -> C -> A -> Root subtrees completed before the matcher reaches completion (1st walk)
  EXPECT_EQ(matcher.on_subtree_count, 4);
  EXPECT_EQ(matcher.walk_count, 1);
  EXPECT_TRUE(matcher.error.empty());
  EXPECT_EQ(seen, 2);
  EXPECT_TRUE(matcher.on_done);
}

TEST_F(MatchTest, OnScanCompetion) {
  size_t seen = 0;
  SingleNodeMatcher<2> matcher("/A/C/D", [&](auto name, const auto& decoder) {
    seen++;
    EXPECT_EQ(name, "D");
  });
  matcher.node_match_result = devicetree::ScanState::kActive;
  matcher.walk_result = devicetree::ScanState::kDone;

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher));

  // This matcher completes after a full walk.
  EXPECT_TRUE(matcher.found);
  // D -> C -> A -> Root subtrees completed before the matcher reaches completion on walk (1st walk)
  EXPECT_EQ(matcher.on_subtree_count, 4);
  // Walk 0: * -> A -> B -> C -> D -> E
  EXPECT_EQ(matcher.visit_count, 6);
  EXPECT_EQ(matcher.walk_count, 1);
  EXPECT_TRUE(matcher.error.empty());
  EXPECT_EQ(seen, 1);
  EXPECT_TRUE(matcher.on_done);
}

TEST_F(MatchTest, OnErrorReturnsFalse) {
  size_t seen = 0;
  SingleNodeMatcher<2> matcher("/A/C/D", [&](auto name, const auto& decoder) {
    seen++;
    EXPECT_EQ(name, "D");
  });
  // Matcher will never be 'Done'.
  matcher.node_match_result = devicetree::ScanState::kActive;
  matcher.walk_result = devicetree::ScanState::kActive;

  auto tree = no_prop_tree();
  EXPECT_FALSE(devicetree::Match(tree, matcher));

  // This matcher completes after a full walk.
  EXPECT_TRUE(matcher.found);
  // Walk 0: * -> A -> B -> C -> D -> E
  // Walk 1: * -> A -> B -> C -> D -> E
  EXPECT_EQ(matcher.visit_count, 12);
  // D -> C -> A -> Root subtrees completed before the matcher on each walk.
  EXPECT_EQ(matcher.on_subtree_count, 8);
  EXPECT_EQ(matcher.walk_count, 2);
  EXPECT_FALSE(matcher.error.empty());
  EXPECT_EQ(seen, 2);
  EXPECT_FALSE(matcher.on_done);
}

TEST_F(MatchTest, MultipleMatchersEarlyCompletion) {
  size_t seen_1 = 0;
  SingleNodeMatcher<2> matcher_1("/A/C/D", [&](auto name, const auto& decoder) {
    seen_1++;
    EXPECT_EQ(name, "D");
  });

  size_t seen_2 = 0;
  SingleNodeMatcher<2> matcher_2("/E/F/G/H", [&](auto name, const auto& decoder) {
    seen_2++;
    EXPECT_EQ(name, "H");
  });

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher_1, matcher_2));

  EXPECT_TRUE(matcher_1.found);
  EXPECT_EQ(matcher_1.visit_count, 5);
  EXPECT_EQ(matcher_1.walk_count, 0);
  EXPECT_EQ(matcher_1.on_subtree_count, 0);
  EXPECT_TRUE(matcher_1.error.empty());
  EXPECT_EQ(seen_1, 1);
  EXPECT_TRUE(matcher_1.on_done);

  EXPECT_TRUE(matcher_2.found);
  EXPECT_EQ(matcher_2.visit_count, 6);
  EXPECT_EQ(matcher_2.walk_count, 0);
  EXPECT_EQ(matcher_2.on_subtree_count, 0);
  EXPECT_TRUE(matcher_2.error.empty());
  EXPECT_EQ(seen_2, 1);
  EXPECT_TRUE(matcher_2.on_done);
}

TEST_F(MatchTest, MultipleMatchersOnScanCompletion) {
  size_t seen_1 = 0;
  SingleNodeMatcher<2> matcher_1("/A/C/D", [&](auto name, const auto& decoder) {
    seen_1++;
    EXPECT_EQ(name, "D");
  });
  matcher_1.node_match_result = devicetree::ScanState::kActive;
  matcher_1.walk_result = devicetree::ScanState::kDone;

  size_t seen_2 = 0;
  SingleNodeMatcher<2> matcher_2("/E/F/G/H", [&](auto name, const auto& decoder) {
    seen_2++;
    EXPECT_EQ(name, "H");
  });
  matcher_2.node_match_result = devicetree::ScanState::kActive;
  matcher_2.walk_result = devicetree::ScanState::kDone;

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher_1, matcher_2));

  EXPECT_TRUE(matcher_1.found);
  EXPECT_EQ(matcher_1.visit_count, 6);
  EXPECT_EQ(matcher_1.walk_count, 1);
  // Only non zero because the matcher is completed on Walk, not on visit.
  EXPECT_EQ(matcher_1.on_subtree_count, 4);
  EXPECT_TRUE(matcher_1.error.empty());
  EXPECT_EQ(seen_1, 1);
  EXPECT_TRUE(matcher_1.on_done);

  EXPECT_TRUE(matcher_2.found);
  EXPECT_EQ(matcher_2.visit_count, 7);
  EXPECT_EQ(matcher_2.walk_count, 1);
  // Only non zero because the matcher is completed on Walk, not on visit.
  EXPECT_EQ(matcher_2.on_subtree_count, 5);
  EXPECT_TRUE(matcher_2.error.empty());
  EXPECT_EQ(seen_2, 1);
  EXPECT_TRUE(matcher_2.on_done);
}

TEST_F(MatchTest, MultipleMatchersOnErrorIsFalse) {
  size_t seen_1 = 0;
  SingleNodeMatcher<2> matcher_1("/A/C/D", [&](auto name, const auto& decoder) {
    seen_1++;
    EXPECT_EQ(name, "D");
  });

  size_t seen_2 = 0;
  SingleNodeMatcher<2> matcher_2("/E/F/G/H", [&](auto name, const auto& decoder) {
    seen_2++;
    EXPECT_EQ(name, "H");
  });
  matcher_2.node_match_result = devicetree::ScanState::kActive;
  matcher_2.walk_result = devicetree::ScanState::kActive;

  auto tree = no_prop_tree();
  EXPECT_FALSE(devicetree::Match(tree, matcher_1, matcher_2));

  EXPECT_TRUE(matcher_1.found);
  EXPECT_EQ(matcher_1.visit_count, 5);
  EXPECT_EQ(matcher_1.walk_count, 0);
  EXPECT_EQ(matcher_1.on_subtree_count, 0);
  EXPECT_TRUE(matcher_1.error.empty());
  EXPECT_EQ(seen_1, 1);
  EXPECT_TRUE(matcher_1.on_done);

  EXPECT_TRUE(matcher_2.found);
  EXPECT_EQ(matcher_2.visit_count, 14);
  EXPECT_EQ(matcher_2.walk_count, 2);
  EXPECT_EQ(matcher_2.on_subtree_count, 10);
  EXPECT_FALSE(matcher_2.error.empty());
  EXPECT_EQ(seen_2, 2);
  EXPECT_FALSE(matcher_2.on_done);
}

TEST_F(MatchTest, OnSubtreeCalledWhenActive) {
  size_t seen_1 = 0;
  size_t root_after = 0;
  SingleNodeMatcher<2> matcher_1(
      "/A/C/D",
      [&](auto name, const auto& decoder) {
        seen_1++;
        EXPECT_EQ(name, "D");
      },
      []() {},
      [&](const devicetree::NodePath& path) {
        if (path == "/A") {
          root_after++;
          return devicetree::ScanState::kDone;
        }
        return devicetree::ScanState::kActive;
      });
  matcher_1.node_match_result = devicetree::ScanState::kActive;

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher_1));

  EXPECT_EQ(root_after, 1);
  EXPECT_EQ(matcher_1.visit_count, 5);
  EXPECT_EQ(matcher_1.walk_count, 0);
  EXPECT_EQ(matcher_1.on_subtree_count, 3);
  EXPECT_TRUE(matcher_1.error.empty());
  EXPECT_EQ(seen_1, 1);
  EXPECT_TRUE(matcher_1.on_done);
}

TEST_F(MatchTest, OnSubtreeDoneWithSubtreeIsNoOp) {
  size_t seen_1 = 0;
  size_t root_after = 0;
  SingleNodeMatcher<2> matcher_1(
      "/A/C/D",
      [&](auto name, const auto& decoder) {
        seen_1++;
        EXPECT_EQ(name, "D");
      },
      []() {},
      [&](const devicetree::NodePath& path) {
        if (path == "/A") {
          root_after++;
          return devicetree::ScanState::kDone;
        }
        // Done with subtree means its not done yet, but nothing else to do with the subtree,
        // in this case should be equivalent to |kDone|.
        return devicetree::ScanState::kDoneWithSubtree;
      });
  matcher_1.node_match_result = devicetree::ScanState::kActive;

  auto tree = no_prop_tree();
  EXPECT_TRUE(devicetree::Match(tree, matcher_1));

  EXPECT_EQ(root_after, 1);
  EXPECT_EQ(matcher_1.visit_count, 5);
  EXPECT_EQ(matcher_1.walk_count, 0);
  EXPECT_EQ(matcher_1.on_subtree_count, 3);
  EXPECT_TRUE(matcher_1.error.empty());
  EXPECT_EQ(seen_1, 1);
  EXPECT_TRUE(matcher_1.on_done);
}

struct OkNodeMatcher {
  static constexpr size_t kMaxScans = 1;
  static constexpr bool kMatchOkNodesOnly = true;

  devicetree::ScanState OnNode(const devicetree::NodePath&,
                               const devicetree::PropertyDecoder& decoder) {
    ok_node_count++;
    return devicetree::ScanState::kActive;
  }
  void OnError(std::string_view v) {}
  devicetree::ScanState OnSubtree(const devicetree::NodePath&) {
    return devicetree::ScanState::kActive;
  }
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }
  void OnDone() {}

  size_t ok_node_count = 0;
};
static_assert(devicetree::OkNodeMatcher<OkNodeMatcher>);

struct NodeMatcher {
  static constexpr size_t kMaxScans = 1;
  static constexpr bool kMatchOkNodesOnly = false;

  devicetree::ScanState OnNode(const devicetree::NodePath&,
                               const devicetree::PropertyDecoder& decoder) {
    node_count++;
    return devicetree::ScanState::kActive;
  }
  void OnError(std::string_view v) {}
  devicetree::ScanState OnSubtree(const devicetree::NodePath&) {
    return devicetree::ScanState::kActive;
  }
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }
  void OnDone() {}

  size_t node_count = 0;
};
static_assert(!devicetree::OkNodeMatcher<NodeMatcher>);

struct NodeMatcher2 {
  static constexpr size_t kMaxScans = 1;

  devicetree::ScanState OnNode(const devicetree::NodePath&,
                               const devicetree::PropertyDecoder& decoder) {
    node_count++;
    return devicetree::ScanState::kActive;
  }
  void OnError(std::string_view v) {}
  devicetree::ScanState OnSubtree(const devicetree::NodePath&) {
    return devicetree::ScanState::kActive;
  }
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }
  void OnDone() {}

  size_t node_count = 0;
};
static_assert(!devicetree::OkNodeMatcher<NodeMatcher>);

TEST(MatchTest, OkNodeMatcherOnlyVisitOkayStatusNodes) {
  //
  //              root
  //     /     /      \    \   \   \
  //    A     C        E    F   G   H
  //   /       \        \
  //  B         D        J
  //
  // From the structure above:
  //  * A,B have no status property and should be treated as "okay"
  //  * C has status "okay".
  //  * D has status "ok" which should be equivalent to "okay".
  //  * E is "disabled", which means J is disabled as well.
  //  * F is "fail", which is not okay.
  //  * G is "fail-123", which is not okay.
  //  * H is "random-value", which is an unknown string and should be not okay.
  //
  auto dtb_with_status = devicetree::testing::LoadDtb("simple_with_status.dtb");
  ASSERT_TRUE(dtb_with_status.is_ok(), "%s", dtb_with_status.error_value().c_str());

  OkNodeMatcher m1;
  NodeMatcher m2;
  NodeMatcher2 m3;

  ASSERT_TRUE(devicetree::Match(dtb_with_status->fdt(), m1, m2, m3));

  EXPECT_EQ(m1.ok_node_count, 5u);
  EXPECT_EQ(m2.node_count, 10u);
  EXPECT_EQ(m2.node_count, m3.node_count);
}

}  // namespace
