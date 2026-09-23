// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_up.h"

#include <gtest/gtest.h>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/console/mock_console.h"
#include "src/developer/debug/zxdb/symbols/file_line.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/mock_source_file_provider.h"
#include "src/developer/debug/zxdb/symbols/mock_symbol_data_provider.h"

namespace zxdb {

namespace {

class VerbUp : public ConsoleTest {};

}  // namespace

TEST_F(VerbUp, Up) {
  std::vector<std::unique_ptr<Frame>> frames;
  constexpr uint64_t kAddress0 = 0x12471253;
  constexpr uint64_t kSP0 = 0x2000;
  frames.push_back(std::make_unique<MockFrame>(
      &session(), thread(), Location(Location::State::kSymbolized, kAddress0), kSP0));

  // Inject a partial stack for an exception the "up" command will have to request more frames.
  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), false);

  // Don't care about the stop notification.
  loop().RunUntilNoTasks();
  console().FlushOutputEvents();

  // This is the reply with the full stack it will get asynchronously. We add three stack
  // frames.
  debug_ipc::ThreadStatusReply thread_status;
  thread_status.record.id = {.process = kProcessKoid, .thread = kThreadKoid};
  thread_status.record.state = debug_ipc::ThreadRecord::State::kBlocked;
  thread_status.record.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  thread_status.record.stack_amount = debug_ipc::ThreadRecord::StackAmount::kFull;
  thread_status.record.frames.emplace_back(kAddress0, kSP0, kSP0);
  thread_status.record.frames.emplace_back(kAddress0 + 16, kSP0 + 16, kSP0 + 16);
  thread_status.record.frames.emplace_back(kAddress0 + 32, kSP0 + 32, kSP0 + 32);

  mock_remote_api()->set_thread_status_reply(thread_status);

  // This will be at frame "0" initially. Going up should take us to from 2, but it will have to
  // request the frames before these can complete which we respond to asynchronously after.
  console().ProcessInputLine("up");
  console().ProcessInputLine("up");

  loop().RunUntilNoTasks();

  auto event = console().GetOutputEvent();
  EXPECT_EQ("Frame 1 0x12471263", event.output.AsString());
  event = console().GetOutputEvent();
  EXPECT_EQ("Frame 2 0x12471273", event.output.AsString());
}

TEST_F(VerbUp, UpWithSource) {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), Location(), 0x1000));

  const char kFileName[] = "file.cc";
  FileLine file_line(kFileName, 2);
  auto source_file_provider = std::make_unique<MockSourceFileProvider>();
  source_file_provider->SetFileData(
      kFileName, SourceFileProvider::FileData("line1\nline2\nline3\nline4", kFileName, 0));
  Location loc(0x1000, file_line, 0, SymbolContext::ForRelativeAddresses(), nullptr);
  auto frame1 = std::make_unique<MockFrame>(&session(), thread(), loc, 0x1000);
  frame1->set_source_file_provider(std::move(source_file_provider));
  frames.push_back(std::move(frame1));

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), /*has_all_frames=*/true);

  console().ProcessInputLine("up");
  loop().RunUntilNoTasks();

  std::string output;
  while (console().HasOutputEvent()) {
    output += console().GetOutputEvent().output.AsString();
  }

  EXPECT_TRUE(debug::StringContains(output, "Frame 1"));
  EXPECT_TRUE(debug::StringContains(output, "▶ 2 line2"));
}

}  // namespace zxdb
