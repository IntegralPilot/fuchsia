// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/ld/fuchsia-debugdata.h>
#include <lib/ld/testing/mock-debugdata.h>

#include <type_traits>

#include <gtest/gtest.h>

#include "debugdata-tests.h"

namespace {

using ::testing::AllOf;

using ld::testing::GetKoid;
using ld::testing::kSinkName;
using ld::testing::kVmoContents;
using ld::testing::kVmoName;
using ld::testing::MakeTestVmo;

static_assert(std::is_default_constructible_v<ld::Debugdata>);
static_assert(std::is_move_constructible_v<ld::Debugdata>);
static_assert(std::is_move_assignable_v<ld::Debugdata>);

TEST(LdTests, DebugdataPublish) {
  // Make a VMO to publish and note its KOID.
  zx::vmo test_vmo;
  ASSERT_NO_FATAL_FAILURE(MakeTestVmo(test_vmo));
  zx_koid_t test_vmo_koid;
  ASSERT_NO_FATAL_FAILURE(GetKoid(test_vmo, test_vmo_koid));

  // Tee it up to be published, also testing move construction and assignment.
  ld::Debugdata debugdata;
  ld::Debugdata moved_from{kSinkName, std::move(test_vmo)};
  debugdata = ld::Debugdata{std::move(moved_from)};

  // Make endpoints for a /svc connection and publish into it before setting up
  // the mock, so the eventpair's KOID is known.
  fidl::ClientEnd<fuchsia_io::Directory> svc_client_end;
  zx::result svc_server_end = fidl::CreateEndpoints(&svc_client_end);
  ASSERT_TRUE(svc_server_end.is_ok()) << svc_server_end.status_string();

  zx::result publish_result = std::move(debugdata).Publish(
      // The call doesn't consume the channel, but here it (the move-only
      // TakeChannel() return value) is consumed at the end of the full
      // expression (the call).
      svc_client_end.TakeChannel().borrow());
  ASSERT_TRUE(publish_result.is_ok()) << publish_result.status_string();
  zx::eventpair test_eventpair = *std::move(publish_result);
  ASSERT_TRUE(test_eventpair.is_valid());

  zx_koid_t test_eventpair_server_koid;
  ASSERT_NO_FATAL_FAILURE(
      GetKoid<&zx_info_handle_basic_t::related_koid>(test_eventpair, test_eventpair_server_koid));

  // Now create and prime the mock.
  auto mock = std::make_unique<::testing::StrictMock<ld::testing::MockDebugdata>>();
  EXPECT_CALL(  //
      *mock, Publish(kSinkName,
                     AllOf(ld::testing::ObjNameMatches(kVmoName),
                           ld::testing::ObjKoidMatches(test_vmo_koid),
                           ld::testing::VmoContentsMatch(kVmoContents)),
                     ld::testing::ObjKoidMatches(test_eventpair_server_koid)));

  // Now move it into the directory.
  ld::testing::MockSvcDirectory svc_dir;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Init());
  ASSERT_NO_FATAL_FAILURE(svc_dir.AddEntry<fuchsia_debugdata::Publisher>(std::move(mock)));

  // The endpoint published to earlier becomes a connection to the mock /svc.
  ASSERT_NO_FATAL_FAILURE(svc_dir.Serve(*std::move(svc_server_end)));

  // Finally, drain all the messages so the server code runs before the test
  // ends and checks the mock's expectations.
  zx_status_t status = svc_dir.loop().RunUntilIdle();
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
}

TEST(LdTests, DebugdataDeferredPublish) {
  zx::vmo test_vmo;
  ASSERT_NO_FATAL_FAILURE(MakeTestVmo(test_vmo));
  zx_koid_t test_vmo_koid;
  ASSERT_NO_FATAL_FAILURE(GetKoid(test_vmo, test_vmo_koid));

  // Publish into the deferred channel.
  zx::result<ld::Debugdata::Deferred> deferred =
      ld::Debugdata{kSinkName, std::move(test_vmo)}.DeferredPublish();
  ASSERT_TRUE(deferred.is_ok()) << deferred.status_string();

  zx_koid_t vmo_token_server_koid;
  ASSERT_NO_FATAL_FAILURE(
      GetKoid<&zx_info_handle_basic_t::related_koid>(deferred->vmo_token, vmo_token_server_koid));

  // Now create and prime the mock.
  auto mock = std::make_unique<::testing::StrictMock<ld::testing::MockDebugdata>>();
  EXPECT_CALL(  //
      *mock, Publish(kSinkName,
                     AllOf(ld::testing::ObjNameMatches(kVmoName),
                           ld::testing::ObjKoidMatches(test_vmo_koid),
                           ld::testing::VmoContentsMatch(kVmoContents)),
                     ld::testing::ObjKoidMatches(vmo_token_server_koid)));

  // Now move it into the directory.
  ld::testing::MockSvcDirectory svc_dir;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Init());
  ASSERT_NO_FATAL_FAILURE(svc_dir.AddEntry<fuchsia_debugdata::Publisher>(std::move(mock)));

  // Get a client end to the mock /svc.
  fidl::ClientEnd<fuchsia_io::Directory> svc_client_end;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Serve(svc_client_end));

  // Forward the deferred channel to (the mock) /svc.
  zx::result<> forward_result = ld::Debugdata::Forward(svc_client_end.channel().borrow(),
                                                       std::move(deferred->svc_server_end));
  EXPECT_TRUE(forward_result.is_ok()) << forward_result.status_string();

  // Finally, drain all the messages so the server code runs before the test
  // ends and checks the mock's expectations.
  zx_status_t status = svc_dir.loop().RunUntilIdle();
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
}

}  // namespace
