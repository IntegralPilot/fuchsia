// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/focus/focus_manager.h"

#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"

namespace focus::test {

enum : zx_koid_t { kNodeA = 1, kNodeB, kNodeC, kNodeD };

using types::ViewRef;
using view_tree::ViewNode;

namespace {

static uint64_t g_next_sequence_number = 1;

// Creates an empty snapshot.
std::shared_ptr<const view_tree::Snapshot> EmptySnapshot() {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;
  return snapshot;
}

// Creates a snapshot with the following one-node topology:
//     A
std::shared_ptr<const view_tree::Snapshot> OneNodeSnapshot() {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;

  snapshot->root = kNodeA;
  auto& view_tree = snapshot->view_tree;
  view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID};

  return snapshot;
}

// Creates a snapshot with the following two-node topology:
//     A
//     |
//     B
std::shared_ptr<view_tree::Snapshot> TwoNodeSnapshot() {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;

  snapshot->root = kNodeA;
  auto& view_tree = snapshot->view_tree;
  view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeB}};
  view_tree[kNodeB] = ViewNode{.parent = kNodeA};

  return snapshot;
}

// Creates a snapshot with the following three-node topology:
//     A
//     |
//     B
//     |
//     C
std::shared_ptr<const view_tree::Snapshot> ThreeNodeSnapshot() {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;

  snapshot->root = kNodeA;
  auto& view_tree = snapshot->view_tree;
  view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeA}};
  view_tree[kNodeB] = ViewNode{.parent = kNodeA, .children = {kNodeC}};
  view_tree[kNodeC] = ViewNode{.parent = kNodeB};

  return snapshot;
}

// Creates a snapshot with the following four-node topology:
//      A
//    /   \
//   B     C
//   |
//   D
std::shared_ptr<const view_tree::Snapshot> FourNodeSnapshot() {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;

  snapshot->root = kNodeA;
  auto& view_tree = snapshot->view_tree;
  view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeB, kNodeC}};
  view_tree[kNodeB] = ViewNode{.parent = kNodeA, .children = {kNodeD}};
  view_tree[kNodeC] = ViewNode{.parent = kNodeA};
  view_tree[kNodeD] = ViewNode{.parent = kNodeB};

  return snapshot;
}

// Creates a snapshot with the following four-node topology, with valid ViewRefs for each node:
//      A
//    /   \
//   B     C
//   |
//   D
//
// TODO(https://fxbug.dev/471250287): ViewRef koids don't match keys in view tree!
std::shared_ptr<const view_tree::Snapshot> FourNodeSnapshotWithViewRefs() {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;

  snapshot->root = kNodeA;
  auto& view_tree = snapshot->view_tree;
  {
    auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
    view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID,
                                 .children = {kNodeB, kNodeC},
                                 .view_ref = std::make_shared<const ViewRef>(std::move(view_ref))};
  }
  {
    auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
    view_tree[kNodeB] = ViewNode{.parent = kNodeA,
                                 .children = {kNodeD},
                                 .view_ref = std::make_shared<const ViewRef>(std::move(view_ref))};
  }
  {
    auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
    view_tree[kNodeC] = ViewNode{.parent = kNodeA,
                                 .view_ref = std::make_shared<const ViewRef>(std::move(view_ref))};
  }
  {
    auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
    view_tree[kNodeD] = ViewNode{.parent = kNodeB,
                                 .view_ref = std::make_shared<const ViewRef>(std::move(view_ref))};
  }

  return snapshot;
}

}  // namespace

class FocusManagerTest : public gtest::TestLoopFixture {
 public:
  FocusManagerTest()
      : snapshot_holder_(std::make_shared<view_tree::SnapshotHolder>()),
        focus_manager_(dispatcher(), snapshot_holder_) {}

 protected:
  std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;
  FocusManager focus_manager_;

 private:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

TEST_F(FocusManagerTest, EmptyTransitions) {
  EXPECT_TRUE(focus_manager_.GetFocusChainForTest().empty());

  // Empty snapshot should not affect the empty focus chain.
  snapshot_holder_->SetSnapshot(EmptySnapshot());
  EXPECT_TRUE(focus_manager_.GetFocusChainForTest().empty());

  // A non-empty snapshot should affect the focus chain.
  snapshot_holder_->SetSnapshot(OneNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  // Submitting the same snapshot again should not change the focus chain.
  snapshot_holder_->SetSnapshot(OneNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));
}

// Tree topology:
//     A
//     |
//     B
TEST_F(FocusManagerTest, FocusTransferDownAllowed) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());

  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//     A
//     |
//     B
TEST_F(FocusManagerTest, FocusTransferToSameNode_ShouldHaveNoEffect) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());

  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));

  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//     A
//     |
//     B
TEST_F(FocusManagerTest, FocusTransferToSelfAllowed) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());

  // Transfer focus to B.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));

  // Transfer focus back to A, on the authority of A.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeA), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));
}

// Tree topology:
//     A
//     |
//     B
TEST_F(FocusManagerTest, FocusTransferUpwardDenied) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());

  // Transfer focus to B.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));

  // Requesting change to A from B should fail and no change should be observed on the focus chain.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeB, kNodeA),
            FocusChangeStatus::kErrorRequesterNotRequestAncestor);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//     A
//     |
//     B
TEST_F(FocusManagerTest, FocusTransfer_ToNonFocusableNode_Denied) {
  auto snapshot = TwoNodeSnapshot();
  snapshot->view_tree.at(kNodeB).is_focusable = false;

  snapshot_holder_->SetSnapshot(snapshot);

  // Transfer focus to B.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB),
            FocusChangeStatus::kErrorRequestCannotReceiveFocus);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));
}

// Tree topology:
//         A
//      /    \
//     B      C
//     |
//     D
TEST_F(FocusManagerTest, BranchedTree) {
  snapshot_holder_->SetSnapshot(FourNodeSnapshot());

  // Transfer focus from A to C.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeC), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeC));

  // Transfer focus from A to D.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeD), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeD));

  // Transfer focus from A to B.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));

  // Transfer focus from B to D.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeB, kNodeD), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeD));
}

// Tree topology:
//         A
//      /    \
//     B      C
//     |
//     D
TEST_F(FocusManagerTest, FocusTransfer_WithRequesterNotInFocusChain_Denied) {
  snapshot_holder_->SetSnapshot(FourNodeSnapshot());

  // Transfer focus from A to C.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeC), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeC));

  // Attempt to transfer focus to D on the authority of B. Should fail since B is not in the focus
  // chain.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeB, kNodeD),
            FocusChangeStatus::kErrorRequesterNotAuthorized);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeC));
}

// Tree topology:
//         A
//      /    \
//     B      C
//     |
//     D
TEST_F(FocusManagerTest, SiblingTransferRequestsDenied) {
  snapshot_holder_->SetSnapshot(FourNodeSnapshot());

  // Setup: Transfer to "D".
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeD), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeD));

  // Transfer request from "B" to "C" denied.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeB, kNodeC),
            FocusChangeStatus::kErrorRequesterNotRequestAncestor);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeD));

  // Transfer request from "D" to "C" denied.
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeD, kNodeC),
            FocusChangeStatus::kErrorRequesterNotRequestAncestor);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeD));
}

// Tree topology:
//     A      A     A
//     |      |
//     B  ->  B  ->    ->
//     |
//     C
TEST_F(FocusManagerTest, ViewRemoval_ShouldShortenFocusChain) {
  snapshot_holder_->SetSnapshot(ThreeNodeSnapshot());

  // Emulate a focus transfer from "A" to "C".
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeC), FocusChangeStatus::kAccept);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeC));

  // Client "C" destroys its view.
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));

  // Client "B" destroys its view.
  snapshot_holder_->SetSnapshot(OneNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  snapshot_holder_->SetSnapshot(EmptySnapshot());
  EXPECT_TRUE(focus_manager_.GetFocusChainForTest().empty());
}

// Tree topology:
//  [] -> A (AutoFocus to B)
//        |
//        B
TEST_F(FocusManagerTest, AutoFocus_BeforeSnapshot) {
  focus_manager_.SetAutoFocusForTest(kNodeA, kNodeB);
  EXPECT_TRUE(focus_manager_.GetFocusChainForTest().empty());

  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//  A  (AutoFocus to B)     A
//                      ->  |
//                          B
// Adding the child after setting it as auto focus should cause focus to move.
TEST_F(FocusManagerTest, AutoFocus_OnNewValidAutoFocusChild) {
  snapshot_holder_->SetSnapshot(OneNodeSnapshot());
  focus_manager_.SetAutoFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//  A      A (AutoFocus to B)
//  |  ->  |
//  B      B
// Setting auto focus after adding the child should cause focus to move.
TEST_F(FocusManagerTest, AutoFocus_WithValidTarget) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  focus_manager_.SetAutoFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//  A
//  |
//  B (AutoFocus to C)
//  |
//  C
// Focus moved due to RequestFocus() should trigger auto focus deferment.
TEST_F(FocusManagerTest, AutoFocus_OnRequestFocus) {
  focus_manager_.SetAutoFocusForTest(kNodeB, kNodeC);
  snapshot_holder_->SetSnapshot(ThreeNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeC));
}

// Tree topology:
//  A                      A
//  |                      |
//  B (AutoFocus to C) ->  B (no AutoFocus)
//  |                      |
//  C                      C
// Same as AutoFocus_OnRequestFocus above, except we unset the auto focus target
// before requesting focus and observe no auto focus deferment happening.
TEST_F(FocusManagerTest, UnsetAutoFocus) {
  focus_manager_.SetAutoFocusForTest(kNodeB, kNodeC);
  snapshot_holder_->SetSnapshot(ThreeNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  focus_manager_.SetAutoFocusForTest(kNodeB, ZX_KOID_INVALID);  // Unset.
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//      A (AutoFocus to B)    A
//    /   \                   |
//   B     C        ->        B
//   |
//   D
// Focus moved to ViewTree changes should trigger auto focus deferment.
TEST_F(FocusManagerTest, AutoFocus_FocusMovedDueToViewTreeChange) {
  // Transfer focus from A to C, then set auto focus from A to B.
  snapshot_holder_->SetSnapshot(FourNodeSnapshot());
  focus_manager_.RequestFocusForTest(kNodeA, kNodeC);
  focus_manager_.SetAutoFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeC));

  // When C disappears the focus should transfer to B.
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//   A (AutoFocus to D)
//   |
//   B (AutoFocus to D)
//   |
//   C (unfocusable)
//   |
//   D (unfocusable)
// Focus loops should resolve themselves to the highest node in the loop:
// A should auto transfer focus to D, which is unfocusable so focus goes to C which is unfocusable
// so focus goes to B which tries to transfer focus back to D, which would create a loop. The
// highest node in the loop is B so focus should remain there.
TEST_F(FocusManagerTest, AutoFocus_LoopShouldLandOnTopMostNode) {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;
  {
    snapshot->root = kNodeA;
    auto& view_tree = snapshot->view_tree;
    view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeB}};
    view_tree[kNodeB] = ViewNode{.parent = kNodeA, .children = {kNodeC}};
    view_tree[kNodeC] = ViewNode{.parent = kNodeB, .children = {kNodeD}, .is_focusable = false};
    view_tree[kNodeD] = ViewNode{.parent = kNodeC, .is_focusable = false};
  }
  snapshot_holder_->SetSnapshot(snapshot);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  focus_manager_.SetAutoFocusForTest(kNodeA, kNodeD);
  focus_manager_.SetAutoFocusForTest(kNodeB, kNodeD);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//      A                      A
//    /   \                  /   \
//   B     C        ->      B     C
//   |                            |
//   D                            D
// Focus moved due to RequestFocus() evaluates auto focus targets against the new view tree
// topology. If the target is no longer a descendant of the requester, it should be ignored.
TEST_F(FocusManagerTest, AutoFocus_ShouldIgnoreTargetIfNoLongerDescendant) {
  snapshot_holder_->SetSnapshot(FourNodeSnapshot());
  // B requests auto focus to D. D is currently a descendant of B in FourNodeSnapshot (A->B->D and
  // A->C).
  focus_manager_.SetAutoFocusForTest(kNodeB, kNodeD);

  // Focus remains on A.
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA));

  // Now change the topology so that D is completely moved under C.
  // Thus, D is no longer a descendant of B.
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;
  {
    snapshot->root = kNodeA;
    auto& view_tree = snapshot->view_tree;
    view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeB, kNodeC}};
    view_tree[kNodeB] = ViewNode{.parent = kNodeA};
    view_tree[kNodeC] = ViewNode{.parent = kNodeA, .children = {kNodeD}};
    view_tree[kNodeD] = ViewNode{.parent = kNodeC};
  }
  snapshot_holder_->SetSnapshot(snapshot);

  // Now request focus to B.
  // B's auto-focus target D is evaluated. Since D is no longer a descendant, it is ignored,
  // and focus resolves on B.
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}

// Tree topology:
//      A                      A
//    /   \                  /   \
//   B     C        ->      B     C
//   |                            |
//   D                            D
// This tests that when focus is actively on an auto-focused target (D via B), and a view tree
// topology change severs the target from the requester's subtree, the repairing process drops
// the invalid auto-focus rule and safely falls back to the original requester.
TEST_F(FocusManagerTest, AutoFocus_ShouldIgnoreTargetDuringRepairIfNoLongerDescendant) {
  snapshot_holder_->SetSnapshot(FourNodeSnapshot());
  focus_manager_.SetAutoFocusForTest(kNodeB, kNodeD);

  // Request focus to B. B automatically delegates to D.
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB, kNodeD));

  // Now change the topology so that D is completely moved under C.
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;
  {
    snapshot->root = kNodeA;
    auto& view_tree = snapshot->view_tree;
    view_tree[kNodeA] = ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeB, kNodeC}};
    view_tree[kNodeB] = ViewNode{.parent = kNodeA};
    view_tree[kNodeC] = ViewNode{.parent = kNodeA, .children = {kNodeD}};
    view_tree[kNodeD] = ViewNode{.parent = kNodeC};
  }

  // Triggering the snapshot update causes RepairFocus to run.
  // The chain [A, B, D] is severed between B and D. Focus falls back to B.
  // B evaluates auto-focus to D, but rejects it because D is no longer a descendant.
  // Therefore, focus remains securely on B.
  snapshot_holder_->SetSnapshot(snapshot);
  EXPECT_THAT(focus_manager_.GetFocusChainForTest(), testing::ElementsAre(kNodeA, kNodeB));
}
class FocusChainTest : public gtest::TestLoopFixture,
                       public fidl::Server<fuchsia_ui_focus::FocusChainListener> {
 public:
  FocusChainTest() = default;

  void OnFocusChange(OnFocusChangeRequest& request,
                     OnFocusChangeCompleter::Sync& completer) override {
    num_focus_chains_received_++;
    last_received_chain_.clear();
    if (request.focus_chain().focus_chain().has_value()) {
      for (const auto& view_ref : request.focus_chain().focus_chain().value()) {
        last_received_chain_.push_back(utils::ExtractKoid(view_ref));
      }
    }

    completer.Reply();
  }

  void RegisterFocusListener(FocusManager& focus_manager) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_focus::FocusChainListener>::Create();
    binding_ = fidl::BindServer(dispatcher(), std::move(server_end), this);
    focus_manager.RegisterFocusChainListener(std::move(client_end));
  }

  std::vector<zx_koid_t> last_received_chain_;
  uint32_t num_focus_chains_received_ = 0;

 protected:
  void SetUp() override {
    gtest::TestLoopFixture::SetUp();
    dispatcher_setter_ =
        std::make_unique<utils::ScopedThreadDispatcherSetter>(dispatcher(), dispatcher());
  }

  void TearDown() override {
    dispatcher_setter_.reset();
    gtest::TestLoopFixture::TearDown();
  }

 private:
  std::unique_ptr<utils::ScopedThreadDispatcherSetter> dispatcher_setter_;
  std::optional<fidl::ServerBindingRef<fuchsia_ui_focus::FocusChainListener>> binding_;
};

TEST_F(FocusChainTest, RegisterBeforeSceneSetup_ShouldReturnEmptyFocusChain) {
  auto snapshot_holder = std::make_shared<view_tree::SnapshotHolder>();
  FocusManager focus_manager(dispatcher(), snapshot_holder);

  RegisterFocusListener(focus_manager);
  RunLoopUntilIdle();
  EXPECT_TRUE(last_received_chain_.empty());
}

// A (AutoFocus B)  A (AutoFocus B)
// |                |
// B       ->       C
//                  |
//                  B
// In this case a View is inserted between A and B, where B is auto focused by A. Normally this
// would cause focus to revert to A as its place in the ViewTree is disturbed, but since A has its
// auto focus set to B focus get returned to B. We now have a situation where the focus chain has
// changed, but focus has not. Observe listeners being updated/not updated accordingly.
TEST_F(FocusChainTest, FocusChainChangedButNotFocus) {
  // Create ViewRefs.
  std::shared_ptr<const ViewRef> view_ref_A;
  std::shared_ptr<const ViewRef> view_ref_B;
  std::shared_ptr<const ViewRef> view_ref_C;
  {
    auto [_, view_ref] = scenic::cpp::ViewRefPair::New();
    view_ref_A = std::make_shared<const ViewRef>(std::move(view_ref));
  }
  {
    auto [_, view_ref] = scenic::cpp::ViewRefPair::New();
    view_ref_B = std::make_shared<const ViewRef>(std::move(view_ref));
  }
  {
    auto [_, view_ref] = scenic::cpp::ViewRefPair::New();
    view_ref_C = std::make_shared<const ViewRef>(std::move(view_ref));
  }

  const zx_koid_t koid_A = view_ref_A->koid();
  const zx_koid_t koid_B = view_ref_B->koid();
  const zx_koid_t koid_C = view_ref_C->koid();

  // Initialize focus manager.
  auto snapshot_holder = std::make_shared<view_tree::SnapshotHolder>();
  FocusManager focus_manager(dispatcher(), snapshot_holder);
  RegisterFocusListener(focus_manager);
  auto [client_vrf, server_vrf] = fidl::Endpoints<fuchsia_ui_views::ViewRefFocused>::Create();
  fidl::Client<fuchsia_ui_views::ViewRefFocused> vrf(std::move(client_vrf), dispatcher());
  focus_manager.RegisterViewRefFocused(koid_B, std::move(server_vrf));
  int view_ref_focused_count = 0;
  vrf->Watch().Then([&view_ref_focused_count](auto& result) {
    ASSERT_TRUE(result.is_ok());
    view_ref_focused_count++;
  });
  focus_manager.SetAutoFocusForTest(koid_A, koid_B);
  RunLoopUntilIdle();
  EXPECT_EQ(num_focus_chains_received_, 1u);

  // Scene 1.
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;
  {
    snapshot->root = koid_A;
    auto& view_tree = snapshot->view_tree;
    view_tree[koid_A] =
        ViewNode{.parent = ZX_KOID_INVALID, .children = {koid_B}, .view_ref = view_ref_A};
    view_tree[koid_B] = ViewNode{.parent = koid_A, .view_ref = view_ref_B};
  }
  snapshot_holder->SetSnapshot(snapshot);
  (void)focus_manager.GetFocusChainForTest();  // Trigger lazy update
  RunLoopUntilIdle();

  EXPECT_EQ(num_focus_chains_received_, 2u);
  EXPECT_THAT(last_received_chain_, testing::ElementsAre(koid_A, koid_B));
  EXPECT_EQ(view_ref_focused_count, 1);

  // Scene 2.
  snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = g_next_sequence_number++;
  {
    snapshot->root = koid_A;
    auto& view_tree = snapshot->view_tree;
    view_tree[koid_A] =
        ViewNode{.parent = ZX_KOID_INVALID, .children = {koid_C}, .view_ref = view_ref_A};
    view_tree[koid_C] = ViewNode{.parent = koid_A, .children = {koid_B}, .view_ref = view_ref_C};
    view_tree[koid_B] = ViewNode{.parent = koid_C, .view_ref = view_ref_B};
  }
  snapshot_holder->SetSnapshot(snapshot);
  (void)focus_manager.GetFocusChainForTest();  // Trigger lazy update
  RunLoopUntilIdle();

  EXPECT_EQ(num_focus_chains_received_, 3u);
  EXPECT_THAT(last_received_chain_, testing::ElementsAre(koid_A, koid_C, koid_B));
  EXPECT_EQ(view_ref_focused_count, 1);
}

// Topology:
//      A
//    /   \
//   B     C
//   |
//   D
TEST_F(FocusChainTest, RegisterAfterSceneSetup_ShouldReturnNonEmptyFocusChain) {
  auto snapshot_holder = std::make_shared<view_tree::SnapshotHolder>();
  FocusManager focus_manager(dispatcher(), snapshot_holder);

  // New view tree should set the focus to root.
  snapshot_holder->SetSnapshot(FourNodeSnapshotWithViewRefs());
  (void)focus_manager.GetFocusChainForTest();  // Trigger lazy update
  RegisterFocusListener(focus_manager);
  RunLoopUntilIdle();
  EXPECT_EQ(num_focus_chains_received_, 1u);
  EXPECT_EQ(last_received_chain_.size(), 1u);
}

// Topology:
//          A
//        /   \
//    -> B     C
//       |
//       D
TEST_F(FocusChainTest, NewSnapshotAfterRegister_ShouldReturnNewFocusChain) {
  auto snapshot_holder = std::make_shared<view_tree::SnapshotHolder>();
  FocusManager focus_manager(dispatcher(), snapshot_holder);

  RegisterFocusListener(focus_manager);
  RunLoopUntilIdle();
  EXPECT_EQ(num_focus_chains_received_, 1u);
  EXPECT_TRUE(last_received_chain_.empty());

  snapshot_holder->SetSnapshot(FourNodeSnapshotWithViewRefs());
  (void)focus_manager.GetFocusChainForTest();  // Trigger lazy update
  RunLoopUntilIdle();
  EXPECT_EQ(num_focus_chains_received_, 2u);
  EXPECT_EQ(last_received_chain_.size(), 1u);
}

// Topology:
//     A           A
//   /   \       /   \
//  B     C  -> B     C
//  |           |
//  D           D
TEST_F(FocusChainTest, SameSnapshotTopologyTwice_ShouldNotSendNewFocusChain) {
  auto snapshot_holder = std::make_shared<view_tree::SnapshotHolder>();
  FocusManager focus_manager(dispatcher(), snapshot_holder);

  snapshot_holder->SetSnapshot(FourNodeSnapshotWithViewRefs());
  RegisterFocusListener(focus_manager);
  RunLoopUntilIdle();
  EXPECT_EQ(num_focus_chains_received_, 1u);

  snapshot_holder->SetSnapshot(FourNodeSnapshotWithViewRefs());
  RunLoopUntilIdle();
  EXPECT_EQ(num_focus_chains_received_, 1u);
}

class FocusManagerInspectTest : public gtest::TestLoopFixture {
 public:
  FocusManagerInspectTest()
      : dispatcher_setter_(dispatcher(), dispatcher()),
        inspector_(),
        snapshot_holder_(std::make_shared<view_tree::SnapshotHolder>()),
        focus_manager_(dispatcher(), snapshot_holder_,
                       inspector_.GetRoot().CreateChild("focus_manager")) {}

  std::vector<uint64_t> GetInspectFocusChain() {
    auto hierarchy = ReadHierarchyFromInspector();
    FX_CHECK(hierarchy);
    auto focus_manager = hierarchy.value().GetByPath({"focus_manager"});
    FX_CHECK(focus_manager);
    auto focus_chain = focus_manager->node().get_property<inspect::UintArrayValue>("focus_chain");
    FX_CHECK(focus_chain);
    return focus_chain->value();
  }

  fpromise::result<inspect::Hierarchy> ReadHierarchyFromInspector() {
    fpromise::result<inspect::Hierarchy> result;
    fpromise::single_threaded_executor exec;
    exec.schedule_task(
        inspect::ReadFromInspector(inspector_).then([&](fpromise::result<inspect::Hierarchy>& res) {
          result = std::move(res);
        }));
    exec.run();

    return result;
  }

  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
  inspect::Inspector inspector_;
  std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;
  FocusManager focus_manager_;
};

// Tree topology:
//     A
//     |
//     B
//     |
//     C
TEST_F(FocusManagerInspectTest, InspectTest) {
  snapshot_holder_->SetSnapshot(ThreeNodeSnapshot());

  // Move focus to "C".
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeC), FocusChangeStatus::kAccept);
  EXPECT_THAT(GetInspectFocusChain(), testing::ElementsAre(kNodeA, kNodeB, kNodeC));

  // Move focus to "B".
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeB), FocusChangeStatus::kAccept);
  EXPECT_THAT(GetInspectFocusChain(), testing::ElementsAre(kNodeA, kNodeB));

  // Move focus to "A"
  EXPECT_EQ(focus_manager_.RequestFocusForTest(kNodeA, kNodeA), FocusChangeStatus::kAccept);
  EXPECT_THAT(GetInspectFocusChain(), testing::ElementsAre(kNodeA));
}

}  // namespace focus::test
