// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/console/commands/verb_up.h"
#include "src/developer/debug/zxdb/console/console_context.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/console/mock_console.h"
#include "src/developer/debug/zxdb/symbols/file_line.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/mock_source_file_provider.h"
#include "src/developer/debug/zxdb/symbols/mock_symbol_data_provider.h"

namespace zxdb {

namespace {

class VerbDownTest : public ConsoleTest {};

}  // namespace

TEST_F(VerbDownTest, Down) {
  std::vector<std::unique_ptr<Frame>> frames;
  constexpr uint64_t kAddress0 = 0x12471253;
  constexpr uint64_t kSP0 = 0x2000;
  frames.push_back(std::make_unique<MockFrame>(
      &session(), thread(), Location(Location::State::kSymbolized, kAddress0), kSP0));

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), false);

  loop().RunUntilNoTasks();
  console().FlushOutputEvents();

  debug_ipc::ThreadStatusReply thread_status;
  thread_status.record.id = {.process = kProcessKoid, .thread = kThreadKoid};
  thread_status.record.state = debug_ipc::ThreadRecord::State::kBlocked;
  thread_status.record.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  thread_status.record.stack_amount = debug_ipc::ThreadRecord::StackAmount::kFull;
  thread_status.record.frames.emplace_back(kAddress0, kSP0, kSP0);
  thread_status.record.frames.emplace_back(kAddress0 + 16, kSP0 + 16, kSP0 + 16);
  thread_status.record.frames.emplace_back(kAddress0 + 32, kSP0 + 32, kSP0 + 32);

  mock_remote_api()->set_thread_status_reply(thread_status);

  // need to setActiveFrameIdForThread() later, thus here we call frames to trigger the syncFrames
  console().ProcessInputLine("frame");
  loop().RunUntilNoTasks();
  console().FlushOutputEvents();

  // now we can do the down w/o fetching the frames from the remote_api
  console().context().SetActiveFrameIdForThread(thread(), 2);
  console().ProcessInputLine("down");
  console().ProcessInputLine("down");
  loop().RunUntilNoTasks();

  auto event = console().GetOutputEvent();
  EXPECT_EQ("Frame 1 0x12471263", event.output.AsString());
  event = console().GetOutputEvent();
  EXPECT_EQ("Frame 0 0x12471253", event.output.AsString());
}

TEST_F(VerbDownTest, DownWithSource) {
  std::vector<std::unique_ptr<Frame>> frames;

  const char kFileName[] = "file.cc";
  FileLine file_line(kFileName, 2);
  auto source_file_provider = std::make_unique<MockSourceFileProvider>();
  source_file_provider->SetFileData(
      kFileName, SourceFileProvider::FileData("line1\nline2\nline3\nline4", kFileName, 0));
  Location loc(0x1000, file_line, 0, SymbolContext::ForRelativeAddresses(), nullptr);
  auto frame0 = std::make_unique<MockFrame>(&session(), thread(), loc, 0x1000);
  frame0->set_source_file_provider(std::move(source_file_provider));
  frames.push_back(std::move(frame0));

  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), Location(), 0x1000));

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), /*has_all_frames=*/true);

  // initially set the active frame to 1
  console().context().SetActiveFrameIdForThread(thread(), 1);

  // now down to frame 0
  console().ProcessInputLine("down");
  loop().RunUntilNoTasks();

  std::string output;
  while (console().HasOutputEvent()) {
    output += console().GetOutputEvent().output.AsString();
  }

  EXPECT_TRUE(debug::StringContains(output, "Frame 0"));
  EXPECT_TRUE(debug::StringContains(output, "▶ 2 line2"));
}

}  // namespace zxdb
