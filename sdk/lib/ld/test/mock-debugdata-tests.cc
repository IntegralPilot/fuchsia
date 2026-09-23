// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.debugdata/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/ld/testing/mock-debugdata.h>
#include <zircon/status.h>

#include <string_view>

#include <gtest/gtest.h>

#include "debugdata-tests.h"

namespace {

using ::testing::AllOf;
using ::testing::Not;

using ld::testing::GetKoid;
using ld::testing::kNotIt;
using ld::testing::kSinkName;
using ld::testing::kVmoContents;
using ld::testing::kVmoName;
using ld::testing::MakeEventPair;
using ld::testing::MakeTestVmo;

class LdMockDebugdataTests : public ::testing::Test {
 public:
  void SetUp() override;

  zx::vmo& test_vmo() { return test_vmo_; }
  zx::eventpair& test_eventpair_client() { return test_eventpair_client_; }
  zx::eventpair& test_eventpair_server() { return test_eventpair_server_; }
  zx_koid_t test_vmo_koid() const { return test_vmo_koid_; }
  zx_koid_t test_eventpair_server_koid() const { return test_eventpair_server_koid_; }

 private:
  zx::vmo test_vmo_;
  zx::eventpair test_eventpair_client_, test_eventpair_server_;
  zx_koid_t test_vmo_koid_, test_eventpair_server_koid_;
};

void LdMockDebugdataTests::SetUp() {
  ASSERT_NO_FATAL_FAILURE(MakeTestVmo(test_vmo_));
  ASSERT_NO_FATAL_FAILURE(GetKoid(test_vmo_, test_vmo_koid_));

  ASSERT_NO_FATAL_FAILURE(MakeEventPair(test_eventpair_client_, test_eventpair_server_));
  ASSERT_NO_FATAL_FAILURE(GetKoid(test_eventpair_server_, test_eventpair_server_koid_));
}

// This is really a test of the matchers more than MockDebugdata since it's
// nothing but a gmock method in a FIDL bindings class.  There's no point in
// testing that the FIDL bindings work at this layer.
TEST_F(LdMockDebugdataTests, MockDebugdata) {
  ::testing::StrictMock<ld::testing::MockDebugdata> mock;
  EXPECT_CALL(mock, Publish(kSinkName,
                            AllOf(ld::testing::ObjNameMatches(kVmoName),
                                  Not(ld::testing::ObjNameMatches(kNotIt)),
                                  ld::testing::ObjKoidMatches(test_vmo_koid()),
                                  Not(ld::testing::ObjKoidMatches(ZX_KOID_INVALID)),
                                  Not(ld::testing::ObjKoidMatches(test_eventpair_server_koid())),
                                  ld::testing::VmoContentsMatch(kVmoContents),
                                  Not(ld::testing::VmoContentsMatch(kNotIt))),
                            AllOf(ld::testing::ObjKoidMatches(test_eventpair_server_koid()),
                                  Not(ld::testing::ObjKoidMatches(ZX_KOID_INVALID)),
                                  Not(ld::testing::ObjKoidMatches(test_vmo_koid())))));
  mock.Publish(kSinkName, std::move(test_vmo()), std::move(test_eventpair_server()));
}

// This tests end-to-end via the FIDL server bindings as an integration test
// for the MockSvcDirectory as well as the MockDebugdata.
TEST_F(LdMockDebugdataTests, MockSvcDirectory) {
  // First create and prime the mock.
  auto mock = std::make_unique<::testing::StrictMock<ld::testing::MockDebugdata>>();
  EXPECT_CALL(*mock, Publish(kSinkName,
                             AllOf(ld::testing::ObjNameMatches(kVmoName),
                                   ld::testing::ObjKoidMatches(test_vmo_koid()),
                                   ld::testing::VmoContentsMatch(kVmoContents)),
                             ld::testing::ObjKoidMatches(test_eventpair_server_koid())));

  // Now move it into the directory.
  ld::testing::MockSvcDirectory svc_dir;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Init());
  ASSERT_NO_FATAL_FAILURE(svc_dir.AddEntry<fuchsia_debugdata::Publisher>(std::move(mock)));

  // Make a channel for the debugdata protocol.
  fidl::ClientEnd<fuchsia_debugdata::Publisher> debugdata_client_end;
  zx::result debugdata_server_end = fidl::CreateEndpoints(&debugdata_client_end);
  ASSERT_TRUE(debugdata_server_end.is_ok()) << debugdata_server_end.status_string();

  // Stuff the Publish call into the client end.
  fidl::SyncClient debugdata_client(std::move(debugdata_client_end));
  auto publish_result = debugdata_client->Publish({{
      .data_sink{kSinkName},
      .data{std::move(test_vmo())},
      .vmo_token{std::move(test_eventpair_server())},
  }});
  ASSERT_TRUE(publish_result.is_ok()) << publish_result.error_value();

  // Send the debugdata server end in the Open call to the directory.
  fidl::ClientEnd<fuchsia_io::Directory> svc_client_end;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Serve(svc_client_end));
  fidl::SyncClient svc_client(std::move(svc_client_end));
  auto open_result = svc_client->Open({{
      .path{fidl::DiscoverableProtocolName<fuchsia_debugdata::Publisher>},
      .flags{fuchsia_io::wire::Flags::kProtocolService},
      .options{},
      .object{debugdata_server_end->TakeChannel()},
  }});
  ASSERT_TRUE(open_result.is_ok()) << open_result.error_value();

  // Now drain the messages so the server code runs before the test ends
  // and checks the mock's expectations.
  zx_status_t status = svc_dir.loop().RunUntilIdle();
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
}

}  // namespace
