// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/console_test.h"

#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"

namespace zxdb {

constexpr uint64_t ConsoleTest::kProcessKoid;
constexpr uint64_t ConsoleTest::kThreadKoid;

void ConsoleTest::SetUp() {
  RemoteAPITest::SetUp();
  console_ = std::make_unique<MockConsole>(&session());
  console_->Init();

  process_ = InjectProcessWithModule(kProcessKoid);
  thread_ = InjectThread(kProcessKoid, kThreadKoid);

  // Eat the output from process attaching (this is asynchronously appended).
  loop().RunUntilNoTasks();
  console_->FlushOutputEvents();
}

void ConsoleTest::TearDown() {
  thread_ = nullptr;
  process_ = nullptr;

  console_.reset();
  RemoteAPITest::TearDown();
}

}  // namespace zxdb
