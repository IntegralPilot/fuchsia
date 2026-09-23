// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/mock_symbol_server.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/developer/debug/zxdb/common/scoped_test_env.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"

namespace zxdb {

namespace {

class VerbSymStat : public ConsoleTest {};

}  // namespace

TEST_F(VerbSymStat, SymStat) {
  // Inject a second module so we can ensure ordering is by load address. The default mock module is
  // inserted with a high load address, so we give a low one.
  InjectMockModule(process(), 0x12345);

  console().ProcessInputLine("sym-stat");

  auto event = console().GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);

  std::string text = event.output.AsString();
  std::stringstream ss(text);

  uint64_t last_addr = 0;
  std::string line;
  while (std::getline(ss, line)) {
    if (line.find("Base: ")) {
      size_t num_start_index = line.find("0x");
      std::string num_string = line.substr(num_start_index);
      uint64_t addr = strtoll(num_string.c_str(), nullptr, 16);
      EXPECT_LT(last_addr, addr);
      last_addr = addr;
    }
  }
}

TEST_F(VerbSymStat, SymStatDownloading) {
  ScopedTestEnv env;
  static auto fake_home =
      std::filesystem::path(GetSelfPath()).parent_path() / "test_data" / "zxdb" / "fake_home";
  ASSERT_EQ(env.Set("HOME", fake_home), 0);
  auto server = std::make_unique<MockSymbolServer>(&session(), "gs://fake-bucket");
  server->InitForTest();
  session().system().InjectSymbolServerForTesting(std::move(server));

  // Make sure this module is marked as not loaded yet. Note that this just marks the mock module as
  // unloaded, but still inserts it into system symbols and the process object itself so that
  // sym-stat can actually see that there is a module there.
  InjectMockModule(process(), 0x12345, "abc123", false);

  session().system().GetDownloadManager()->InjectDownloadForTesting("abc123");

  console().FlushOutputEvents();
  console().ProcessInputLine("sym-stat");

  auto event = console().GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);

  auto text = event.output.AsString();
  EXPECT_TRUE(debug::StringContains(text, "Process 1 symbol status"));
  EXPECT_TRUE(debug::StringContains(text, "Build ID: abc123 (Downloading...)"));

  // Releasing the download will cause it to register a failure.
  session().system().GetDownloadManager()->AbandonTestingDownload("abc123");

  console().ProcessInputLine("sym-stat");

  event = console().GetOutputEvent();
  EXPECT_FALSE(debug::StringContains(event.output.AsString(), "Build ID: abc123 (Downloading...)"));
}

}  // namespace zxdb
