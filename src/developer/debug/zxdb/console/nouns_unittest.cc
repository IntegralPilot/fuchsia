// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_settings.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/console/mock_console.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/mock_source_file_provider.h"

namespace zxdb {

namespace {

class NounsTest : public RemoteAPITest {};

TEST_F(NounsTest, BreakpointList) {
  MockConsole console(&session());
  console.Init();

  const char kListBreakpointsLine[] = "bp";
  const char kListBreakpointsVerboseLine[] = "bp -v";

  // List breakpoints when there are none.
  console.ProcessInputLine(kListBreakpointsLine);
  auto event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ("No breakpoints.\n", event.output.AsString());

  // Create a breakpoint with no settings.
  const char kExpectedNoSettings[] =
      " # scope  stop enabled type      #addrs hit-count location\n"
      " 1 global all  true    software pending           <no location>\n";
  Breakpoint* bp = session().system().CreateNewBreakpoint();
  console.ProcessInputLine(kListBreakpointsLine);
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(kExpectedNoSettings, event.output.AsString());

  // Verbose list, there are no locations so this should be the same.
  console.ProcessInputLine(kListBreakpointsVerboseLine);
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(kExpectedNoSettings, event.output.AsString());

  // Set location.
  BreakpointSettings in;
  in.enabled = false;
  in.locations.emplace_back(Identifier(IdentifierComponent("Foo")));
  bp->SetSettings(in);

  // List breakpoints now that there are settings.
  console.ProcessInputLine(kListBreakpointsLine);
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(
      " # scope  stop enabled type      #addrs hit-count location\n"
      " 1 global all  false   software pending           Foo\n",
      event.output.AsString());

  // Add a non-software breakpoint to see the size.
  Breakpoint* write_bp = session().system().CreateNewBreakpoint();
  BreakpointSettings write_settings;
  write_settings.type = BreakpointSettings::Type::kWrite;
  write_settings.byte_size = 4;
  write_settings.locations.emplace_back(0x12345678);
  write_bp->SetSettings(write_settings);

  console.ProcessInputLine(kListBreakpointsLine);
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(
      " # scope  stop enabled type     size  #addrs hit-count location\n"
      " 1 global all  false   software  n/a pending           Foo\n"
      " 2 global all  true    write       4 pending           0x12345678\n",
      event.output.AsString());

  // Currently we don't test printing breakpoint locations since that requires
  // injecting a mock process with mock symbols. If we add infrastructure for
  // other noun tests to do this such that this can be easily written, we
  // should add something here.
}

TEST_F(NounsTest, FilterTest) {
  MockConsole console(&session());
  console.Init();

  console.ProcessInputLine("filter");
  auto event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ("No filters.\n", event.output.AsString());

  console.ProcessInputLine("filter attach foo");
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ("\"filter\" may not be specified for this command.", event.output.AsString());

  console.ProcessInputLine("attach foobar");
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(
      "Waiting for process matching \"foobar\".\n"
      "Type \"filter\" to see the current filters.",
      event.output.AsString());

  console.ProcessInputLine("attach --job 1 boofar");
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(
      "Waiting for process matching \"boofar\".\n"
      "Type \"filter\" to see the current filters.",
      event.output.AsString());

  console.ProcessInputLine("filter");
  event = console.GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);
  ASSERT_EQ(
      "  # Type                Pattern Job \n"
      "  1 process name substr foobar      \n"
      "▶ 2 process name substr boofar    1 \n",
      event.output.AsString());
}

class NounTestWithThread : public ConsoleTest {};

// Let NounTestWithThread initialize the thread for further stack injection.
TEST_F(NounTestWithThread, FrameNoun) {
  std::vector<std::unique_ptr<Frame>> frames;

  constexpr uint64_t kAddress0 = 0x1234;
  constexpr uint64_t kAddress1 = 0x5678;

  {
    const char kFileName[] = "file0.cc";
    FileLine file_line(kFileName, 1);
    auto source_file_provider = std::make_unique<MockSourceFileProvider>();
    source_file_provider->SetFileData(
        kFileName, SourceFileProvider::FileData("writing\na\nunittest\n", kFileName, 0));
    Location loc(kAddress0, file_line, 0, SymbolContext::ForRelativeAddresses(), nullptr);
    auto frame = std::make_unique<MockFrame>(&session(), thread(), loc, 0x2000);
    frame->set_source_file_provider(std::move(source_file_provider));
    frames.push_back(std::move(frame));
  }

  {
    const char kFileName[] = "file1.cc";
    FileLine file_line(kFileName, 2);
    auto source_file_provider = std::make_unique<MockSourceFileProvider>();
    source_file_provider->SetFileData(
        kFileName, SourceFileProvider::FileData("is\nnot\nhard\n", kFileName, 0));
    Location loc(kAddress1, file_line, 0, SymbolContext::ForRelativeAddresses(), nullptr);
    auto frame = std::make_unique<MockFrame>(&session(), thread(), loc, 0x2000);
    frame->set_source_file_provider(std::move(source_file_provider));
    frames.push_back(std::move(frame));
  }

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), /*has_all_frames=*/true);

  loop().RunUntilNoTasks();
  console().FlushOutputEvents();

  // It should show all frames with filenames.
  console().ProcessInputLine("frame");
  loop().RunUntilNoTasks();
  std::string output = console().GetOutputEvent().output.AsString();
  EXPECT_TRUE(debug::StringContains(output, "▶ 0") && debug::StringContains(output, "file0.cc"));
  EXPECT_TRUE(debug::StringContains(output, "1") && debug::StringContains(output, "file1.cc"));

  console().ProcessInputLine("frame 0");
  loop().RunUntilNoTasks();
  output = "";
  while (console().HasOutputEvent()) {
    output += console().GetOutputEvent().output.AsString();
  }

  EXPECT_TRUE(debug::StringContains(output, "file0.cc"));
  EXPECT_TRUE(debug::StringContains(output, "▶ 1 writing"));
  EXPECT_TRUE(debug::StringContains(output, "2 a"));
  EXPECT_TRUE(debug::StringContains(output, "3 unittest"));

  console().ProcessInputLine("frame 1");
  loop().RunUntilNoTasks();
  output = "";
  while (console().HasOutputEvent()) {
    output += console().GetOutputEvent().output.AsString();
  }
  EXPECT_TRUE(debug::StringContains(output, "file1.cc"));
  EXPECT_TRUE(debug::StringContains(output, "1 is"));
  EXPECT_TRUE(debug::StringContains(output, "▶ 2 not"));
  EXPECT_TRUE(debug::StringContains(output, "3 hard"));
}

}  // namespace
}  // namespace zxdb
