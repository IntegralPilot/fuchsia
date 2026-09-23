// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/view_ref_installed_impl.h"

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

namespace view_tree::test {

TEST(ViewRefInstalledImplTest, AlreadyInstalled_ShouldReturnImmediately) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  auto snapshot_holder = std::make_shared<SnapshotHolder>();
  ViewRefInstalledImpl view_ref_installed_impl(snapshot_holder);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
  const zx_koid_t koid = utils::ExtractKoid(view_ref);

  // Koid is in the ViewTree.
  auto snapshot = std::make_shared<Snapshot>();
  (void)snapshot->view_tree[koid];
  snapshot->sequence_number = 1;
  snapshot_holder->SetSnapshot(snapshot);
  view_ref_installed_impl.OnNewViewTreeSnapshot();

  bool was_installed = false;
  client->Watch({{.view_ref = std::move(view_ref)}})
      .ThenExactlyOnce(
          [&was_installed](fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
            was_installed = result.is_ok();
          });

  test_loop.RunUntilIdle();
  EXPECT_TRUE(was_installed);
}

TEST(ViewRefInstalledImplTest, AlreadyInstalledButDisconnected_ShouldReturnImmediately) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  auto snapshot_holder = std::make_shared<SnapshotHolder>();
  ViewRefInstalledImpl view_ref_installed_impl(snapshot_holder);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
  const zx_koid_t koid = utils::ExtractKoid(view_ref);

  {  // Koid is in the ViewTree.
    auto snapshot = std::make_shared<Snapshot>();
    (void)snapshot->view_tree[koid];
    snapshot->sequence_number = 1;
    snapshot_holder->SetSnapshot(snapshot);
    view_ref_installed_impl.OnNewViewTreeSnapshot();
  }

  {  // Koid is unconnected.
    auto snapshot = std::make_shared<Snapshot>();
    snapshot->unconnected_views.emplace(koid);
    snapshot->sequence_number = 2;
    snapshot_holder->SetSnapshot(snapshot);
    view_ref_installed_impl.OnNewViewTreeSnapshot();
  }

  bool was_installed = false;
  client->Watch({{.view_ref = std::move(view_ref)}})
      .ThenExactlyOnce(
          [&was_installed](fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
            was_installed = result.is_ok();
          });

  test_loop.RunUntilIdle();
  EXPECT_TRUE(was_installed);
}

TEST(ViewRefInstalledImplTest, ViewRefWithBadHandle_ShouldReturnErrorImmediately) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  ViewRefInstalledImpl view_ref_installed_impl;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  // Create an uninitialized ViewRef.
  fuchsia_ui_views::ViewRef view_ref;

  bool was_error = false;
  client->Watch({{.view_ref = std::move(view_ref)}})
      .ThenExactlyOnce(
          [&was_error](fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
            was_error = result.is_error();
          });
  test_loop.RunUntilIdle();
  EXPECT_TRUE(was_error);
}

TEST(ViewRefInstalledImplTest, ViewRefWithBadRights_ShouldReturnErrorImmediately) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  ViewRefInstalledImpl view_ref_installed_impl;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  // Create a ViewRefPair where the ViewRef has faulty rights.
  auto view_pair = scenic::cpp::ViewRefPair::New();
  zx::eventpair replaced_eventpair;
  auto status = view_pair.view_ref.reference().replace(ZX_RIGHT_INSPECT, &replaced_eventpair);
  ASSERT_EQ(status, ZX_OK);
  fuchsia_ui_views::ViewRef faulty_view_ref(std::move(replaced_eventpair));

  bool was_error = false;
  client->Watch({{.view_ref = std::move(faulty_view_ref)}})
      .ThenExactlyOnce(
          [&was_error](fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
            was_error = result.is_error();
          });
  test_loop.RunUntilIdle();
  EXPECT_TRUE(was_error);
}

TEST(ViewRefInstalledImplTest, ViewRefWithClosedControlRef_ShouldReturnErrorImmediately) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  ViewRefInstalledImpl view_ref_installed_impl;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  // Create a ViewRefPair and close the ViewRefControl before passing in the ViewRef.
  auto view_pair = scenic::cpp::ViewRefPair::New();
  view_pair.control_ref.reference().reset();

  bool was_error = false;
  client->Watch({{.view_ref = std::move(view_pair.view_ref)}})
      .ThenExactlyOnce(
          [&was_error](fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
            was_error = result.is_error();
          });
  test_loop.RunUntilIdle();
  EXPECT_TRUE(was_error);
}

TEST(ViewRefInstalledImplTest, OnViewRefInstalled_ShouldFireWaitingCallbacks) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  auto snapshot_holder = std::make_shared<SnapshotHolder>();
  ViewRefInstalledImpl view_ref_installed_impl(snapshot_holder);
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
  const zx_koid_t koid = utils::ExtractKoid(view_ref);

  bool has_fired = false;
  bool was_error = false;
  client->Watch({{.view_ref = std::move(view_ref)}})
      .ThenExactlyOnce([&has_fired, &was_error](
                           fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
        has_fired = true;
        was_error = result.is_error();
      });
  test_loop.RunUntilIdle();
  EXPECT_FALSE(has_fired);

  // Submit a new snapshot where the koid is in the ViewTree.
  auto snapshot = std::make_shared<Snapshot>();
  (void)snapshot->view_tree[koid];
  snapshot->sequence_number = 1;
  snapshot_holder->SetSnapshot(snapshot);
  view_ref_installed_impl.OnNewViewTreeSnapshot();

  test_loop.RunUntilIdle();
  EXPECT_TRUE(has_fired);
  EXPECT_FALSE(was_error);
}

TEST(ViewRefInstalledImplTest, OnViewRefInvalidated_ShouldFireCallbackWithError) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  auto snapshot_holder = std::make_shared<SnapshotHolder>();
  ViewRefInstalledImpl view_ref_installed_impl(snapshot_holder);
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  bool has_fired = false;
  bool was_error = false;
  {
    auto view_pair = scenic::cpp::ViewRefPair::New();
    client->Watch({{.view_ref = std::move(view_pair.view_ref)}})
        .ThenExactlyOnce([&has_fired, &was_error](
                             fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
          has_fired = true;
          was_error = result.is_error();
        });
    test_loop.RunUntilIdle();
    EXPECT_FALSE(has_fired);
  }  // ViewRefControl goes out of scope, invalidating the passed in ViewRef.
  test_loop.RunUntilIdle();
  EXPECT_TRUE(has_fired);
  EXPECT_TRUE(was_error);
}

TEST(ViewRefInstalledImplTest, InstalledThenInvalidated) {
  async::TestLoop test_loop;
  utils::ScopedThreadDispatcherSetter dispatcher_setter(test_loop.dispatcher(),
                                                        test_loop.dispatcher());

  auto snapshot_holder = std::make_shared<SnapshotHolder>();
  ViewRefInstalledImpl view_ref_installed_impl(snapshot_holder);
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::ViewRefInstalled>::Create();
  view_ref_installed_impl.Bind(std::move(server_end));
  fidl::Client<fuchsia_ui_views::ViewRefInstalled> client(std::move(client_end),
                                                          test_loop.dispatcher());

  bool has_fired = false;
  bool was_error = false;

  {
    auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
    const zx_koid_t koid = utils::ExtractKoid(view_ref);

    client->Watch({{.view_ref = std::move(view_ref)}})
        .ThenExactlyOnce([&has_fired, &was_error](
                             fidl::Result<fuchsia_ui_views::ViewRefInstalled::Watch>& result) {
          has_fired = true;
          was_error = result.is_error();
        });
    test_loop.RunUntilIdle();
    EXPECT_FALSE(has_fired);

    // Submit a new snapshot where the koid is in the ViewTree.
    async::PostTask(test_loop.dispatcher(), [&view_ref_installed_impl, snapshot_holder, koid] {
      auto snapshot = std::make_shared<Snapshot>();
      (void)snapshot->view_tree[koid];
      snapshot->sequence_number = 1;
      snapshot_holder->SetSnapshot(snapshot);
      view_ref_installed_impl.OnNewViewTreeSnapshot();
    });
  }  // ViewRefControl goes out of scope, invalidating the passed in ViewRef.

  // Two things are now on the dispatch queue:
  // 1. OnNewViewTreeSnapshot(), which should trigger OnViewRefInstalled().
  // 2. ViewRef invalidation, which should trigger OnViewRefInvalidated().
  // Observe that this is handled gracefully.
  test_loop.RunUntilIdle();
  EXPECT_TRUE(has_fired);
  EXPECT_FALSE(was_error);
}

}  // namespace view_tree::test
