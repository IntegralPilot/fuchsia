// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstddef>
#include <memory>

#include <gtest/gtest.h>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/console/mock_console.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/symbols/mock_source_file_provider.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace zxdb {

namespace {

class VerbListTest : public ConsoleTest {};

}  // namespace

TEST_F(VerbListTest, ListSource) {
  const char kFileName[] = "file.cc";
  auto source_file_provider = std::make_unique<MockSourceFileProvider>();
  source_file_provider->SetFileData(
      kFileName, SourceFileProvider::FileData("line1\nline2\nline3\nline4", kFileName, 0));
  FileLine file_line(kFileName, 2);
  Location loc(0x1000, file_line, 0, SymbolContext::ForRelativeAddresses(), nullptr);

  std::vector<std::unique_ptr<Frame>> frames;
  auto frame = std::make_unique<MockFrame>(&session(), thread(), loc, 0x2000);
  frame->set_source_file_provider(std::move(source_file_provider));
  frames.push_back(std::move(frame));
  thread()->GetStack().SetFramesForTest(std::move(frames), true);

  console().ProcessInputLine("list");
  auto event = console().GetOutputEvent();
  EXPECT_TRUE(debug::StringContains(event.output.AsString(), "1 line1"));
  EXPECT_TRUE(debug::StringContains(event.output.AsString(), "▶ 2 line2"));
  EXPECT_TRUE(debug::StringContains(event.output.AsString(), "3 line3"));
  EXPECT_TRUE(debug::StringContains(event.output.AsString(), "4 line4"));
}

}  // namespace zxdb
