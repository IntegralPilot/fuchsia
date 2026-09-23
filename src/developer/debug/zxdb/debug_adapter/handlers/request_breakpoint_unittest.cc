// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <filesystem>
#include <fstream>
#include <vector>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_settings.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestBreakpointTest : public DebugAdapterContextTest {
 public:
  void SetUp() override {
    DebugAdapterContextTest::SetUp();
    char temp_dir_template[] = "/tmp/zxdb_dap_test_XXXXXX";
    char* temp_dir = mkdtemp(temp_dir_template);
    ASSERT_TRUE(temp_dir);
    temp_dir_path_ = temp_dir;
    file_path_ = temp_dir_path_ / "test.cc";
    std::ofstream(file_path_).close();
  }

  void TearDown() override {
    std::filesystem::remove_all(temp_dir_path_);
    DebugAdapterContextTest::TearDown();
  }

  std::filesystem::path temp_dir_path_;
  std::filesystem::path file_path_;
};

}  // namespace

TEST_F(RequestBreakpointTest, SetBreakpoints) {
  InitializeDebugging();

  // Send breakpoint request from the client.
  dap::SetBreakpointsRequest req = {};
  req.source.name = "i2c.c";
  req.source.path = file_path_.string();
  req.lines = {30, 64};
  req.breakpoints = {{.line = 30}, {.line = 64}};
  auto response = client().send(req);

  // Read request and process it.
  context().OnStreamReadable();

  // Run client to receive response.
  RunClient();
  auto got = response.get();
  EXPECT_EQ(got.error, false);
  EXPECT_EQ(got.response.breakpoints.size(), req.breakpoints.value().size());
  EXPECT_EQ(got.response.breakpoints[0].line.value(), req.lines.value()[0]);
  EXPECT_EQ(got.response.breakpoints[1].line.value(), req.lines.value()[1]);
  EXPECT_EQ(got.response.breakpoints[0].source.value().name.value(), req.source.name.value());
}

TEST_F(RequestBreakpointTest, UpdateBreakpoints) {
  InitializeDebugging();

  // Send breakpoint request from the client.
  dap::SetBreakpointsRequest req = {};
  req.source.path = file_path_.string();
  req.lines = {30, 40, 50};
  req.breakpoints = {{.line = 30}, {.line = 40}, {.line = 50}};
  auto response = client().send(req);

  // Read request and process it.
  context().OnStreamReadable();

  // Run client to receive response.
  RunClient();
  auto got = response.get();
  EXPECT_EQ(got.error, false);
  EXPECT_EQ(got.response.breakpoints.size(), req.breakpoints.value().size());
  EXPECT_EQ(context().GetBreakpointsForSource(file_path_)->size(), req.breakpoints.value().size());

  // Remove a breakpoint and send request again. Old breakpoints should be replaced with the new
  // ones for the source file.
  req.lines = {40, 50};
  req.breakpoints = {{.line = 40}, {.line = 50}};
  auto updated_response = client().send(req);

  context().OnStreamReadable();
  RunClient();
  got = updated_response.get();
  EXPECT_EQ(got.error, false);
  EXPECT_EQ(got.response.breakpoints.size(), req.breakpoints.value().size());
  EXPECT_EQ(got.response.breakpoints[0].line.value(), req.lines.value()[0]);
  EXPECT_EQ(got.response.breakpoints[1].line.value(), req.lines.value()[1]);
  EXPECT_EQ(context().GetBreakpointsForSource(file_path_)->size(), req.breakpoints.value().size());
}

TEST_F(RequestBreakpointTest, SetBreakpointsWithNonCanonicalPath) {
  InitializeDebugging();

  // "//foo/../test.cc" can be canonicalized fully, since its parent exists on disk.
  std::filesystem::create_directory(temp_dir_path_ / "foo");
  std::filesystem::path non_canonical_path_foo = temp_dir_path_ / "foo" / ".." / "test.cc";

  // "//bar/../test.cc" can only be weakly canonicalized, since its parent does not exists on disk.
  std::filesystem::path non_canonical_path_bar = temp_dir_path_ / "bar" / ".." / "test.cc";

  // Send breakpoint request from the client with non-canonical path.
  dap::SetBreakpointsRequest req = {};
  req.source.path = non_canonical_path_foo.string();
  req.lines = {10};
  req.breakpoints = {{.line = 10}};
  auto response = client().send(req);

  // Read request and process it.
  context().OnStreamReadable();

  // Run client to receive response.
  RunClient();
  auto got = response.get();
  EXPECT_EQ(got.error, false);
  EXPECT_EQ(got.response.breakpoints.size(), 1u);

  // The context should have stored the breakpoint against the canonical path.
  EXPECT_NE(context().GetBreakpointsForSource(file_path_), nullptr);
  EXPECT_EQ(context().GetBreakpointsForSource(file_path_)->size(), 1u);
  EXPECT_EQ(context().GetBreakpointsForSource(non_canonical_path_foo), nullptr);

  // Send another request to clear breakpoints, using a different non-canonical path.
  req = {};
  req.source.path = non_canonical_path_bar.string();
  auto clear_response = client().send(req);
  context().OnStreamReadable();
  RunClient();
  auto clear_got = clear_response.get();
  EXPECT_EQ(clear_got.error, false);
  EXPECT_EQ(clear_got.response.breakpoints.size(), 0u);
  EXPECT_EQ(context().GetBreakpointsForSource(file_path_), nullptr);
}

TEST_F(RequestBreakpointTest, SetBreakpointsWithRelativePath) {
  InitializeDebugging();

  dap::SetBreakpointsRequest req = {};
  req.source.path = "some/relative/path.cc";
  req.lines = {10};
  req.breakpoints = {{.line = 10}};
  auto response = client().send(req);

  context().OnStreamReadable();

  RunClient();
  auto got = response.get();
  EXPECT_EQ(got.error, true);
  EXPECT_EQ(got.response.breakpoints.size(), 0u);
}

TEST_F(RequestBreakpointTest, SetBreakpointsWithNoPathSet) {
  InitializeDebugging();

  dap::SetBreakpointsRequest req = {};
  req.source.name = "test.cc";
  req.source.sourceReference = 42;
  req.lines = {10};
  req.breakpoints = {{.line = 10}};
  auto response = client().send(req);

  context().OnStreamReadable();

  RunClient();
  auto got = response.get();
  EXPECT_EQ(got.error, true);
  EXPECT_EQ(got.response.breakpoints.size(), 0u);
}

TEST_F(RequestBreakpointTest, SetBreakpointsWithCondition) {
  InitializeDebugging();

  dap::SetBreakpointsRequest req = {};
  req.source.path = file_path_.string();
  req.lines = {10};
  req.breakpoints = {{.condition = "y == 100", .line = 10}};
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 1u);
  auto bps = context().GetBreakpointsForSource(file_path_);
  ASSERT_NE(bps, nullptr);
  ASSERT_EQ(bps->size(), 1u);
  ASSERT_TRUE((*bps)[0]);
  EXPECT_EQ((*bps)[0]->GetSettings().condition, "y == 100");
}

}  // namespace zxdb
