// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string.h>

#include <string_view>

#include <gtest/gtest.h>

#include "cmdline.h"

namespace gigaboot {
namespace {

TEST(CommandlineTest, EmptyToString) {
  Commandline cmdline;
  char buffer[64];
  auto res = cmdline.ToString({buffer, sizeof(buffer)});
  ASSERT_TRUE(res.is_ok());
  ASSERT_EQ(res.value(), 0UL);
}

TEST(CommandlineTest, Add) {
  char buffer[64];
  Commandline cmdline;

  ASSERT_TRUE(cmdline.Add("kname", "fuchsia"));
  ASSERT_TRUE(cmdline.Add("presence_flag"));
  ASSERT_TRUE(cmdline.Add("kregion", "1000"));
  ASSERT_TRUE(cmdline.Add("second_flag"));

  auto res = cmdline.ToString({buffer, sizeof(buffer)});
  ASSERT_TRUE(res.is_ok());
  ASSERT_EQ(res.value(), 52UL);

  ASSERT_EQ(std::string_view(buffer, strlen(buffer)),
            "kname=fuchsia presence_flag kregion=1000 second_flag");
}

TEST(CommandlineTest, OverrideAdd) {
  char buffer[64];
  Commandline cmdline;

  ASSERT_TRUE(cmdline.Add("kname", "fuchsia"));
  ASSERT_TRUE(cmdline.Add("kname", "linux"));

  auto res = cmdline.ToString({buffer, sizeof(buffer)});
  ASSERT_TRUE(res.is_ok());

  ASSERT_EQ(std::string_view(buffer, strlen(buffer)), "kname=linux");
}

// This function needs to be constexpr (or return a static array) so that the
// name strings aren't dangling pointers.
constexpr std::array<char, Commandline::kMaxCmdlineItems> VarNames() {
  std::array<char, Commandline::kMaxCmdlineItems> names = {};
  for (uint64_t i = 0; i < std::size(names); ++i) {
    // Some of these names will be unprintable, but that doesn't matter.
    names[i] = static_cast<char>('A' + i);
  }

  return names;
}

TEST(CommandlineTest, AddFailureTooManyKeys) {
  Commandline cmdline;

  constexpr std::array<char, Commandline::kMaxCmdlineItems> names = VarNames();
  for (auto& name : names) {
    ASSERT_TRUE(cmdline.Add(std::string_view(&name, 1)));
  }

  // Commandline struct is full.
  ASSERT_FALSE(cmdline.Add("key", "val"));

  // But even when full we can revalue existing entries.
  ASSERT_TRUE(cmdline.Add("A", "aardvark"));
  ASSERT_EQ(cmdline.Get("A"), "aardvark");
}

TEST(CommandlineTest, AddFailureTooBig) {
  Commandline cmdline;

  ASSERT_FALSE(cmdline.Add(std::string_view("flag", Commandline::kCmdlineMaxArgSize + 1)));
  ASSERT_FALSE(cmdline.Add("key", std::string_view("val", Commandline::kCmdlineMaxArgSize + 1)));
}

TEST(CommandlineTest, AppendItems) {
  Commandline cmdline;
  char buffer[64];

  std::string_view cmd = " key=val  flag kname=fuchsia second_flag  ";
  ASSERT_TRUE(cmdline.AppendItems(cmd));
  ASSERT_EQ(cmdline.Get("key"), "val");
  ASSERT_EQ(cmdline.Get("kname"), "fuchsia");
  ASSERT_EQ(cmdline.Get("nonexist"), "");
  ASSERT_EQ(cmdline.Get("nonexist", "default"), "default");

  auto res = cmdline.ToString({buffer, sizeof(buffer)});
  ASSERT_TRUE(res.is_ok());
  std::string_view text(buffer);
  ASSERT_EQ(res.value(), text.size());
  ASSERT_EQ(text, "key=val flag kname=fuchsia second_flag");
}

}  // namespace
}  // namespace gigaboot
