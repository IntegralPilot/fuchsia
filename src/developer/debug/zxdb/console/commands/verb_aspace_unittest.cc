// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/console/console_test.h"

namespace zxdb {

namespace {

class VerbAspace : public ConsoleTest {};

constexpr std::vector<debug_ipc::AddressRegion> GetCommonMappings() {
  return {
      // The process's vmar.
      {
          .name = "test1",
          .base = 0x200000,
          .size = 0x80000000000 - 0x200000,
          .depth = 0,
      },
      // The process's "root" vmar.
      {
          .name = "test1-root",
          .base = 0x200000,
          .size = 0x80000000000 - 0x200000,
          .depth = 1,
      },
      // The allocator's root vmar.
      {
          .name = "useralloc",
          .base = 0x200000,
          .size = 0x80000000000 - 0x200000,
          .depth = 2,
      },
      // Now all the actual mappings. We'll start with the low-address-mapped mappings.
      {
          .name = "blob-1234",
          .base = 0x954e4000,
          .size = 0x4000,
          .depth = 3,
      },
      {
          .name = "blob-1234",
          .base = 0x954e8000,
          .size = 0x1000,
          .depth = 3,
      },
      {
          .name = "blob-1234",
          .base = 0x954e9000,
          .size = 0x2000,
          .depth = 3,
      },
      // Now for some high-address-mapped mappings. Technically these should probably be in a
      // separate vmar-like setup from the above, but this should be fine for tests.
      {
          .name = "blob-b16add12s",
          .base = 0x800307d91000,
          .size = 0x4000,
          .depth = 3,
      },
      {
          .name = "blob-b16add12s",
          .base = 0x800307d95000,
          .size = 0x1000,
          .depth = 3,
      },
      {
          .name = "blob-b16add12s",
          .base = 0x800307d96000,
          .size = 0x1000,
          .depth = 3,
      },
  };
}

}  // namespace

TEST_F(VerbAspace, Aspace) {
  debug_ipc::AddressSpaceReply reply;
  reply.map = GetCommonMappings();

  mock_remote_api()->set_address_space_reply(reply);

  console().ProcessInputLine("aspace");

  auto event = console().GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);

  std::string text = event.output.AsString();
  std::stringstream output_text(text);
  std::string line;

  // Get the column specification line first.
  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "Start"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x200000"));
  EXPECT_TRUE(debug::StringContains(line, "0x80000000000"));
  EXPECT_TRUE(debug::StringContains(line, "test1"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x200000"));
  EXPECT_TRUE(debug::StringContains(line, "0x80000000000"));
  EXPECT_TRUE(debug::StringContains(line, "test1-root"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x200000"));
  EXPECT_TRUE(debug::StringContains(line, "0x80000000000"));
  EXPECT_TRUE(debug::StringContains(line, "useralloc"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x954e4000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-1234"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x954e8000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-1234"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x954e9000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-1234"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x800307d91000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-b16add12s"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x800307d95000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-b16add12s"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x800307d96000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-b16add12s"));
}

TEST_F(VerbAspace, AspaceLimit) {
  debug_ipc::AddressSpaceReply reply;
  reply.map = GetCommonMappings();

  mock_remote_api()->set_address_space_reply(reply);

  console().ProcessInputLine("aspace --limit 0x100000000");
  auto event = console().GetOutputEvent();
  ASSERT_EQ(MockConsole::OutputEvent::Type::kOutput, event.type);

  std::string text = event.output.AsString();
  std::stringstream output_text(text);
  std::string line;

  // Get the column specification line first.
  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "Start"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x200000"));
  EXPECT_TRUE(debug::StringContains(line, "0x80000000000"));
  EXPECT_TRUE(debug::StringContains(line, "test1"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x200000"));
  EXPECT_TRUE(debug::StringContains(line, "0x80000000000"));
  EXPECT_TRUE(debug::StringContains(line, "test1-root"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x200000"));
  EXPECT_TRUE(debug::StringContains(line, "0x80000000000"));
  EXPECT_TRUE(debug::StringContains(line, "useralloc"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x954e4000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-1234"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x954e8000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-1234"));

  ASSERT_TRUE(std::getline(output_text, line));
  EXPECT_TRUE(debug::StringContains(line, "0x954e9000"));
  EXPECT_TRUE(debug::StringContains(line, "blob-1234"));

  // Eat the last two lines, which are a newline and the page size. The committed pages and mapped
  // bytes summary are not included when using "--limit".
  ASSERT_TRUE(std::getline(output_text, line));
  ASSERT_TRUE(std::getline(output_text, line));
  ASSERT_FALSE(std::getline(output_text, line));
}

}  // namespace zxdb
