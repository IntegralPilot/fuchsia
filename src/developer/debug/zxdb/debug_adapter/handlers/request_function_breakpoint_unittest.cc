// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <vector>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_settings.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"

namespace zxdb {

namespace {

class RequestFunctionBreakpointTest : public DebugAdapterContextTest {};

}  // namespace

TEST_F(RequestFunctionBreakpointTest, SetFunctionBreakpoints) {
  InitializeDebugging();

  dap::SetFunctionBreakpointsRequest req = {};
  req.breakpoints = {
      dap::FunctionBreakpoint{.name = "main"},
      dap::FunctionBreakpoint{.name = "MyNamespace::MyClass::MyFunc"},
  };
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 2u);
  ASSERT_EQ(context().GetFunctionBreakpoints().size(), 2u);
  EXPECT_TRUE(got.response.breakpoints[0].id.has_value());
  EXPECT_TRUE(got.response.breakpoints[1].id.has_value());
}

TEST_F(RequestFunctionBreakpointTest, UpdateFunctionBreakpoints) {
  InitializeDebugging();

  dap::SetFunctionBreakpointsRequest req = {};
  req.breakpoints = {
      dap::FunctionBreakpoint{.name = "FuncA"},
      dap::FunctionBreakpoint{.name = "FuncB"},
      dap::FunctionBreakpoint{.name = "FuncC"},
  };
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 3u);
  ASSERT_EQ(context().GetFunctionBreakpoints().size(), 3u);

  // Send another request with fewer breakpoints to replace existing ones.
  req.breakpoints = {
      dap::FunctionBreakpoint{.name = "FuncB"},
  };
  auto updated_response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  got = updated_response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 1u);
  ASSERT_EQ(context().GetFunctionBreakpoints().size(), 1u);
}

TEST_F(RequestFunctionBreakpointTest, ClearFunctionBreakpoints) {
  InitializeDebugging();

  dap::SetFunctionBreakpointsRequest req = {};
  req.breakpoints = {
      dap::FunctionBreakpoint{.name = "FuncA"},
      dap::FunctionBreakpoint{.name = "FuncB"},
  };
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 2u);
  ASSERT_EQ(context().GetFunctionBreakpoints().size(), 2u);

  // Clear all function breakpoints by sending an empty list.
  req.breakpoints = {};
  auto clear_response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto clear_got = clear_response.get();
  EXPECT_FALSE(clear_got.error);
  EXPECT_EQ(clear_got.response.breakpoints.size(), 0u);
  EXPECT_TRUE(context().GetFunctionBreakpoints().empty());
}

TEST_F(RequestFunctionBreakpointTest, SetFunctionBreakpointsWithCondition) {
  InitializeDebugging();

  dap::SetFunctionBreakpointsRequest req = {};
  req.breakpoints = {
      dap::FunctionBreakpoint{.condition = "x == 42", .name = "main"},
  };
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 1u);
  ASSERT_EQ(context().GetFunctionBreakpoints().size(), 1u);

  auto bp = context().GetFunctionBreakpoints()[0];
  ASSERT_TRUE(bp);
  EXPECT_EQ(bp->GetSettings().condition, "x == 42");
}

TEST_F(RequestFunctionBreakpointTest, SetFunctionBreakpointsWithInvalidName) {
  InitializeDebugging();

  dap::SetFunctionBreakpointsRequest req = {};
  req.breakpoints = {
      dap::FunctionBreakpoint{.name = "valid_func"},
      dap::FunctionBreakpoint{.name = "invalid<unclosed"},
  };
  auto response = client().send(req);

  context().OnStreamReadable();
  RunClient();

  auto got = response.get();
  EXPECT_FALSE(got.error);
  ASSERT_EQ(got.response.breakpoints.size(), 2u);
  EXPECT_TRUE(got.response.breakpoints[0].id.has_value());
  EXPECT_FALSE(got.response.breakpoints[1].verified);
  EXPECT_TRUE(got.response.breakpoints[1].message.has_value());
  EXPECT_EQ(context().GetFunctionBreakpoints().size(), 1u);
}

TEST_F(RequestFunctionBreakpointTest, FunctionAndSourceBreakpointsIsolation) {
  InitializeDebugging();

  // Set source breakpoints.
  dap::SetBreakpointsRequest src_req = {};
  src_req.source.path = "/fake/path/test.cc";
  src_req.lines = {10, 20};
  src_req.breakpoints = {{.line = 10}, {.line = 20}};
  auto src_response = client().send(src_req);
  context().OnStreamReadable();
  RunClient();
  EXPECT_FALSE(src_response.get().error);
  EXPECT_EQ(context().GetBreakpointsForSource("/fake/path/test.cc")->size(), 2u);

  // Set function breakpoints.
  dap::SetFunctionBreakpointsRequest fn_req = {};
  fn_req.breakpoints = {dap::FunctionBreakpoint{.name = "my_func"}};
  auto fn_response = client().send(fn_req);
  context().OnStreamReadable();
  RunClient();
  EXPECT_FALSE(fn_response.get().error);
  EXPECT_EQ(context().GetFunctionBreakpoints().size(), 1u);

  // Verify source breakpoints are still intact.
  EXPECT_EQ(context().GetBreakpointsForSource("/fake/path/test.cc")->size(), 2u);

  // Clear function breakpoints.
  fn_req.breakpoints = {};
  auto clear_fn_response = client().send(fn_req);
  context().OnStreamReadable();
  RunClient();
  EXPECT_FALSE(clear_fn_response.get().error);
  EXPECT_TRUE(context().GetFunctionBreakpoints().empty());

  // Source breakpoints must still remain.
  EXPECT_EQ(context().GetBreakpointsForSource("/fake/path/test.cc")->size(), 2u);
}

}  // namespace zxdb
