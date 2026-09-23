// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <optional>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/focus/focus_manager.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

// This test exercises the implementation of the fuchsia.ui.views.ViewRefFocused protocol, which
// allows clients to listen to view-focus events.
//
// Visual geometry is not important in this test. We use the following two-node tree topology:
//   A
//   |
//   B
namespace {

enum : zx_koid_t { kNodeA = 1, kNodeB };

// Class fixture for TEST_F.
class ViewRefFocusedTest : public gtest::TestLoopFixture {
 protected:
  // Creates a snapshot with the following two-node topology:
  //     A
  //     |
  //     B
  std::shared_ptr<const view_tree::Snapshot> TwoNodeSnapshot() {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    snapshot->sequence_number = next_sequence_number_++;

    snapshot->root = kNodeA;
    auto& view_tree = snapshot->view_tree;
    view_tree[kNodeA] = view_tree::ViewNode{.parent = ZX_KOID_INVALID, .children = {kNodeB}};
    view_tree[kNodeB] = view_tree::ViewNode{.parent = kNodeA};

    return snapshot;
  }

  ViewRefFocusedTest()
      : snapshot_holder_(std::make_shared<view_tree::SnapshotHolder>()),
        focus_manager_(dispatcher(), snapshot_holder_) {}

  void SetUp() override {
    gtest::TestLoopFixture::SetUp();
    dispatcher_setter_ =
        std::make_unique<utils::ScopedThreadDispatcherSetter>(dispatcher(), dispatcher());
    auto [client_a, server_a] = fidl::Endpoints<fuchsia_ui_views::ViewRefFocused>::Create();
    node_a_focused_.Bind(std::move(client_a), dispatcher());
    focus_manager_.RegisterViewRefFocused(kNodeA, std::move(server_a));

    auto [client_b, server_b] = fidl::Endpoints<fuchsia_ui_views::ViewRefFocused>::Create();
    node_b_focused_.Bind(std::move(client_b), dispatcher());
    focus_manager_.RegisterViewRefFocused(kNodeB, std::move(server_b));

    FX_CHECK(node_a_focused_.is_valid());
    FX_CHECK(node_b_focused_.is_valid());
  }

  void TearDown() override {
    dispatcher_setter_.reset();
    gtest::TestLoopFixture::TearDown();
  }

  std::unique_ptr<utils::ScopedThreadDispatcherSetter> dispatcher_setter_;
  std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;
  focus::FocusManager focus_manager_;
  fidl::Client<fuchsia_ui_views::ViewRefFocused> node_a_focused_;
  fidl::Client<fuchsia_ui_views::ViewRefFocused> node_b_focused_;

 private:
  uint64_t next_sequence_number_ = 1;
};

TEST_F(ViewRefFocusedTest, NoFocus_NoResponse) {
  // No snapshots declared yet, "empty scene".

  bool called_node_a = false;
  node_a_focused_->Watch().Then([&called_node_a](auto&) { called_node_a = true; });

  bool called_node_b = false;
  node_b_focused_->Watch().Then([&called_node_b](auto&) { called_node_b = true; });

  RunLoopUntilIdle();
  EXPECT_FALSE(called_node_a);
  EXPECT_FALSE(called_node_b);
}

TEST_F(ViewRefFocusedTest, BasicTree_ParentGetsFocus) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  (void)focus_manager_.GetFocusChainForTest();  // Trigger lazy update

  std::optional<bool> node_a_focus;
  node_a_focused_->Watch().Then(
      [&node_a_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_a_focus = result->state().focused().value();
      });

  bool called_node_b = false;
  node_b_focused_->Watch().Then([&called_node_b](auto&) { called_node_b = true; });

  RunLoopUntilIdle();
  ASSERT_TRUE(node_a_focus.has_value());  // received a focus event
  EXPECT_TRUE(node_a_focus.value());      // node A has focus
  EXPECT_FALSE(called_node_b);
}

TEST_F(ViewRefFocusedTest, ChildFocus_FalseToTrue) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());

  // Poll after node B gains focus.
  std::optional<bool> node_b_focus;
  node_b_focused_->Watch().Then(
      [&node_b_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_b_focus = result->state().focused().value();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(node_b_focus.has_value());

  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);

  RunLoopUntilIdle();
  ASSERT_TRUE(node_b_focus.has_value());
  EXPECT_TRUE(node_b_focus.value());
}

TEST_F(ViewRefFocusedTest, ChildFocus_FalseToFalse) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);
  focus_manager_.RequestFocusForTest(kNodeA, kNodeA);

  // Poll after node B gains then loses focus.
  std::optional<bool> node_b_focus;
  node_b_focused_->Watch().Then(
      [&node_b_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_b_focus = result->state().focused().value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(node_b_focus.has_value());
  EXPECT_FALSE(node_b_focus.value());
}

TEST_F(ViewRefFocusedTest, ChildFocus_TrueToFalse) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);

  // First poll by node B sees focus gained.
  std::optional<bool> node_b_focus;
  node_b_focused_->Watch().Then(
      [&node_b_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_b_focus = result->state().focused().value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(node_b_focus.has_value());
  EXPECT_TRUE(node_b_focus.value());

  // Second poll by node B sees focus lost.
  node_b_focus.reset();
  node_b_focused_->Watch().Then(
      [&node_b_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_b_focus = result->state().focused().value();
      });

  focus_manager_.RequestFocusForTest(kNodeA, kNodeA);

  RunLoopUntilIdle();
  ASSERT_TRUE(node_b_focus.has_value());
  EXPECT_FALSE(node_b_focus.value());
}

TEST_F(ViewRefFocusedTest, ChildFocus_TrueToTrue) {
  snapshot_holder_->SetSnapshot(TwoNodeSnapshot());
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);

  // First poll by node B sees focus gained.
  std::optional<bool> node_b_focus;
  node_b_focused_->Watch().Then(
      [&node_b_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_b_focus = result->state().focused().value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(node_b_focus.has_value());
  EXPECT_TRUE(node_b_focus.value());

  // Second poll by node B sees focus lost then gained.
  node_b_focus.reset();
  node_b_focused_->Watch().Then(
      [&node_b_focus](fidl::Result<fuchsia_ui_views::ViewRefFocused::Watch>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->state().focused().has_value());
        node_b_focus = result->state().focused().value();
      });

  focus_manager_.RequestFocusForTest(kNodeA, kNodeA);
  focus_manager_.RequestFocusForTest(kNodeA, kNodeB);

  RunLoopUntilIdle();
  ASSERT_TRUE(node_b_focus.has_value());
  EXPECT_TRUE(node_b_focus.value());
}

}  // namespace
