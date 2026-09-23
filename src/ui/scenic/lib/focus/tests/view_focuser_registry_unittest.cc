// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/focus/view_focuser_registry.h"

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"

namespace focus::test {

constexpr zx_koid_t kFocuserKoid = 1;
constexpr zx_koid_t kFocuser2Koid = 2;
constexpr zx_koid_t kRandomKoid = 1124124214;

TEST(ViewFocuserRegistryTest, SuccessfulRequestFocus_ShouldReturnOK) {
  async::TestLoop test_loop;

  auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
  const zx_koid_t view_ref_koid = utils::ExtractKoid(view_ref);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
  fidl::Client<fuchsia_ui_views::Focuser> focuser(std::move(client_end), test_loop.dispatcher());

  ViewFocuserRegistry registry(
      /*request_focus*/
      [=](zx_koid_t requester, zx_koid_t request) {
        EXPECT_EQ(requester, kFocuserKoid);
        EXPECT_EQ(request, view_ref_koid);
        return true;
      },
      /*set_auto_focus*/ [](auto...) { FAIL() << "Unexpected call to set_auto_focus"; },
      test_loop.dispatcher());
  registry.Register(kFocuserKoid, std::move(server_end));
  test_loop.RunUntilIdle();

  std::optional<fidl::Result<fuchsia_ui_views::Focuser::RequestFocus>> result;
  focuser->RequestFocus({{.view_ref = std::move(view_ref)}}).Then([&result](auto& res) {
    result = std::move(res);
  });
  test_loop.RunUntilIdle();

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_ok());
}

TEST(ViewFocuserRegistryTest, FailedRequestFocus_ShouldReturnError) {
  async::TestLoop test_loop;

  auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
  const zx_koid_t view_ref_koid = utils::ExtractKoid(view_ref);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
  fidl::Client<fuchsia_ui_views::Focuser> focuser(std::move(client_end), test_loop.dispatcher());

  ViewFocuserRegistry registry(
      /*request_focus*/
      [=](zx_koid_t requester, zx_koid_t request) {
        EXPECT_EQ(requester, kFocuserKoid);
        EXPECT_EQ(request, view_ref_koid);
        return false;
      },
      /*set_auto_focus*/ [](auto...) { FAIL() << "Unexpected call to set_auto_focus"; },
      test_loop.dispatcher());
  registry.Register(kFocuserKoid, std::move(server_end));
  test_loop.RunUntilIdle();

  std::optional<fidl::Result<fuchsia_ui_views::Focuser::RequestFocus>> result;
  focuser->RequestFocus({{.view_ref = std::move(view_ref)}}).Then([&result](auto& res) {
    result = std::move(res);
  });
  test_loop.RunUntilIdle();

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value().domain_error(), fuchsia_ui_views::Error::kDenied);
}

TEST(ViewFocuserRegistryTest, SetAutoFocus_ShouldCallClosure) {
  async::TestLoop test_loop;

  auto [control_ref, view_ref] = scenic::cpp::ViewRefPair::New();
  const zx_koid_t view_ref_koid = utils::ExtractKoid(view_ref);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
  fidl::Client<fuchsia_ui_views::Focuser> focuser(std::move(client_end), test_loop.dispatcher());

  zx_koid_t last_auto_focus_requester = kRandomKoid;
  zx_koid_t last_auto_focus_target = kRandomKoid;
  ViewFocuserRegistry registry(
      /*request_focus*/ [](auto...) { return false; },
      /*set_auto_focus*/
      [&](zx_koid_t requester, zx_koid_t target) {
        last_auto_focus_requester = requester;
        last_auto_focus_target = target;
      },
      test_loop.dispatcher());
  registry.Register(kFocuserKoid, std::move(server_end));
  test_loop.RunUntilIdle();

  fuchsia_ui_views::FocuserSetAutoFocusRequest request;
  request.view_ref(std::move(view_ref));

  bool callback_received = false;
  std::optional<fidl::Result<fuchsia_ui_views::Focuser::SetAutoFocus>> result;
  focuser->SetAutoFocus(std::move(request)).Then([&callback_received, &result](auto& res) {
    callback_received = true;
    result = std::move(res);
  });
  test_loop.RunUntilIdle();

  EXPECT_EQ(last_auto_focus_requester, kFocuserKoid);
  EXPECT_EQ(last_auto_focus_target, view_ref_koid);
  EXPECT_TRUE(callback_received);

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_ok());
}

TEST(ViewFocuserRegistryTest, Empty_SetAutoFocus_ShouldCallClosureWithInvalidKoid) {
  async::TestLoop test_loop;

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
  fidl::Client<fuchsia_ui_views::Focuser> focuser(std::move(client_end), test_loop.dispatcher());

  zx_koid_t last_auto_focus_requester = kRandomKoid;
  zx_koid_t last_auto_focus_target = kRandomKoid;
  ViewFocuserRegistry registry(
      /*request_focus*/ [](auto...) { return false; },
      /*set_auto_focus*/
      [&](zx_koid_t requester, zx_koid_t target) {
        last_auto_focus_requester = requester;
        last_auto_focus_target = target;
      },
      test_loop.dispatcher());
  registry.Register(kFocuserKoid, std::move(server_end));
  test_loop.RunUntilIdle();

  bool callback_received = false;
  std::optional<fidl::Result<fuchsia_ui_views::Focuser::SetAutoFocus>> result;
  focuser->SetAutoFocus({}).Then([&callback_received, &result](auto& res) {
    callback_received = true;
    result = std::move(res);
  });
  test_loop.RunUntilIdle();

  EXPECT_EQ(last_auto_focus_requester, kFocuserKoid);
  EXPECT_EQ(last_auto_focus_target, ZX_KOID_INVALID);
  EXPECT_TRUE(callback_received);

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_ok());
}

TEST(ViewFocuserRegistryTest, OnChannelClosure_EndpointShouldBeCleanedUp) {
  async::TestLoop test_loop;

  // Register two focusers.
  zx_koid_t last_auto_focus_requester = kRandomKoid;
  zx_koid_t last_auto_focus_target = kRandomKoid;
  ViewFocuserRegistry registry(
      /*request_focus*/ [](auto...) { return true; },
      /*set_auto_focus*/
      [&](zx_koid_t requester, zx_koid_t target) {
        last_auto_focus_requester = requester;
        last_auto_focus_target = target;
      },
      test_loop.dispatcher());
  EXPECT_TRUE(registry.endpoints().empty());

  fidl::Client<fuchsia_ui_views::Focuser> focuser1;
  {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
    focuser1.Bind(std::move(client_end), test_loop.dispatcher());
    registry.Register(kFocuserKoid, std::move(server_end));
  }
  test_loop.RunUntilIdle();
  EXPECT_EQ(registry.endpoints().size(), 1u);
  EXPECT_TRUE(registry.endpoints().count(kFocuserKoid) == 1);

  EXPECT_EQ(last_auto_focus_requester, kRandomKoid);
  EXPECT_EQ(last_auto_focus_target, kRandomKoid);

  fidl::Client<fuchsia_ui_views::Focuser> focuser2;
  {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
    focuser2.Bind(std::move(client_end), test_loop.dispatcher());
    registry.Register(kFocuser2Koid, std::move(server_end));
  }
  test_loop.RunUntilIdle();
  EXPECT_EQ(registry.endpoints().size(), 2u);
  EXPECT_TRUE(registry.endpoints().count(kFocuser2Koid) == 1);

  EXPECT_EQ(last_auto_focus_requester, kRandomKoid);
  EXPECT_EQ(last_auto_focus_target, kRandomKoid);

  // Close one and watch it clean up.
  focuser1 = {};
  test_loop.RunUntilIdle();
  EXPECT_EQ(registry.endpoints().size(), 1u);
  EXPECT_TRUE(registry.endpoints().count(kFocuserKoid) == 0);
  EXPECT_EQ(last_auto_focus_requester, kFocuserKoid);
  EXPECT_EQ(last_auto_focus_target, ZX_KOID_INVALID);

  // Close the other one.
  focuser2 = {};
  test_loop.RunUntilIdle();
  EXPECT_TRUE(registry.endpoints().empty());
  EXPECT_EQ(last_auto_focus_requester, kFocuser2Koid);
  EXPECT_EQ(last_auto_focus_target, ZX_KOID_INVALID);
}

}  // namespace focus::test
