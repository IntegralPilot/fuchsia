// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_attach.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestAttachTest : public DebugAdapterContextTest {};

}  // namespace

TEST_F(RequestAttachTest, AttachKoid) {
  InitializeDebugging();

  // Send attach request with numeric KOID (Process ID)
  dap::AttachRequestZxdb req = {};
  req.process = "12345";
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // Verify that it triggered target->Attach() natively and did NOT create any filters
  auto filters = context().session()->system().GetFilters();
  EXPECT_EQ(filters.size(), 0u);

  auto targets = context().session()->system().GetTargets();
  ASSERT_EQ(targets.size(), 1u);
  EXPECT_EQ(targets[0]->GetState(), Target::State::kAttaching);
}

TEST_F(RequestAttachTest, AttachProcessName) {
  InitializeDebugging();

  // Send attach request with standard process name substring pattern
  dap::AttachRequestZxdb req = {};
  req.process = "test";
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // Verify direct API filter creation
  auto filters = context().session()->system().GetFilters();
  ASSERT_EQ(filters.size(), 1u);
  EXPECT_EQ(filters[0]->pattern(), "test");
  EXPECT_EQ(filters[0]->type(), debug_ipc::Filter::Type::kProcessNameSubstr);
}

TEST_F(RequestAttachTest, AttachComponentName) {
  InitializeDebugging();

  // Send attach request with component manifest manifest name ending in .cm
  dap::AttachRequestZxdb req = {};
  req.process = "my_component.cm";
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // Verify component name filter type
  auto filters = context().session()->system().GetFilters();
  ASSERT_EQ(filters.size(), 1u);
  EXPECT_EQ(filters[0]->pattern(), "my_component.cm");
  EXPECT_EQ(filters[0]->type(), debug_ipc::Filter::Type::kComponentName);
}

TEST_F(RequestAttachTest, AttachRecursive) {
  InitializeDebugging();

  // Send attach request with moniker suffix and recursive flag set
  dap::AttachRequestZxdb req = {};
  req.process = "realm/component";
  req.recursive = true;
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();
  auto got = response.get();
  EXPECT_FALSE(got.error);

  // Verify recursive moniker suffix filter type
  auto filters = context().session()->system().GetFilters();
  ASSERT_EQ(filters.size(), 1u);
  EXPECT_EQ(filters[0]->pattern(), "realm/component");
  EXPECT_EQ(filters[0]->type(), debug_ipc::Filter::Type::kComponentMonikerSuffix);
  EXPECT_TRUE(filters[0]->recursive());
}

TEST_F(RequestAttachTest, AttachEmptyPattern) {
  InitializeDebugging();

  // Send invalid empty attach request
  dap::AttachRequestZxdb req = {};
  req.process = "";
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();
  auto got = response.get();

  // Verify immediate fail-fast validation returned a protocol error
  EXPECT_TRUE(got.error);
}

TEST_F(RequestAttachTest, AttachDuplicateDeduplication) {
  InitializeDebugging();

  // Send first attach request
  dap::AttachRequestZxdb req1 = {};
  req1.process = "test_component.cm";
  auto response1 = client().send(req1);

  context().OnStreamReadable();
  RunClient();
  EXPECT_FALSE(response1.get().error);

  // Send identical duplicate attach request
  dap::AttachRequestZxdb req2 = {};
  req2.process = "test_component.cm";
  auto response2 = client().send(req2);

  context().OnStreamReadable();
  RunClient();
  EXPECT_FALSE(response2.get().error);

  // Verify that the duplicate request reused the existing filter instead of leaking a new one
  auto filters = context().session()->system().GetFilters();
  EXPECT_EQ(filters.size(), 1u);
}

TEST_F(RequestAttachTest, MultipleFiltersTearDown) {
  InitializeDebugging();

  // Send first attach request
  dap::AttachRequestZxdb req1 = {};
  req1.process = "proc1";
  client().send(req1);
  context().OnStreamReadable();
  RunClient();

  // Send second attach request
  dap::AttachRequestZxdb req2 = {};
  req2.process = "proc2";
  client().send(req2);
  context().OnStreamReadable();
  RunClient();

  // Send third attach request
  dap::AttachRequestZxdb req3 = {};
  req3.process = "proc3";
  client().send(req3);
  context().OnStreamReadable();
  RunClient();

  // Verify all 3 filters are created
  auto filters = context().session()->system().GetFilters();
  EXPECT_EQ(filters.size(), 3u);

  // Teardown will now occur when the test exits, verifying that
  // DeleteAllFilters() does not crash when multiple filters are present.
}

}  // namespace zxdb
