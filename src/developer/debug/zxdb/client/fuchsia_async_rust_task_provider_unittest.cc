// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/fuchsia_async_rust_task_provider.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/symbols/function.h"

namespace zxdb {

namespace {

class RustAsyncTaskProviderTest : public RemoteAPITest {
 public:
  RustAsyncTaskProviderTest() = default;
};

TEST_F(RustAsyncTaskProviderTest, CanHandle) {
  FuchsiaAsyncRustTaskProvider provider;

  auto check_can_handle = [&](const std::string& name, bool expected) {
    auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
    func->set_assigned_name(name);
    Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);
    MockFrame frame(nullptr, nullptr, loc, 0);
    EXPECT_EQ(expected, provider.CanHandle(&frame)) << "For function: " << name;
  };

  check_can_handle("fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run", true);
  check_can_handle("fuchsia_async::runtime::fuchsia::executor::send::SendExecutor::run", true);
  check_can_handle(
      "fuchsia_async::runtime::fuchsia::executor::send::create_worker_threads::{closure}", true);
  check_can_handle("some::other::function", false);
}

}  // namespace

}  // namespace zxdb
