// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/program_shortname.h"

#include <map>
#include <optional>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace forensics::crash_reports {
namespace {

TEST(Shorten, ShortensCorrectly) {
  const std::map<std::string, std::string> name_to_shortened_name = {
      // Does nothing.
      {"system", "system"},
      // Remove leading whitespace.
      {"    system", "system"},
      // Remove trailing whitespace.
      {"system    ", "system"},
      // Remove "fuchsia-pkg://" prefix.
      {"fuchsia-pkg://fuchsia.com/foo-bar#meta/foo_bar.cm", "fuchsia.com:foo-bar#meta:foo_bar.cm"},
      // Remove leading whitespace and "fuchsia-pkg://" prefix.
      {"     fuchsia-pkg://fuchsia.com/foo-bar#meta/foo_bar.cm",
       "fuchsia.com:foo-bar#meta:foo_bar.cm"},
      // Replaces runs of '/' with a single ':'.
      {"//////////test/", ":test:"},
  };

  for (const auto& [name, shortend_name] : name_to_shortened_name) {
    const std::optional<ProgramShortname> valid = ProgramShortname::Create(name);
    ASSERT_TRUE(valid.has_value());
    EXPECT_EQ(valid->Value(), shortend_name);
  }
}

TEST(ProgramShortname, Valid) {
  const std::optional<ProgramShortname> valid = ProgramShortname::Create("system");
  ASSERT_TRUE(valid.has_value());
  EXPECT_EQ(valid->Value(), "system");
}

TEST(ProgramShortname, PkgPrefixOnlyInvalid) {
  const std::optional<ProgramShortname> invalid = ProgramShortname::Create("fuchsia-pkg://");
  EXPECT_FALSE(invalid.has_value());
}

TEST(ProgramShortname, WhitespaceOnlyInvalid) {
  const std::optional<ProgramShortname> invalid = ProgramShortname::Create("   ");
  EXPECT_FALSE(invalid.has_value());
}

TEST(ProgramShortname, DotInvalid) {
  EXPECT_FALSE(ProgramShortname::Create(".").has_value());
  EXPECT_FALSE(ProgramShortname::Create("   .   ").has_value());
}

TEST(ProgramShortname, DotDotInvalid) {
  EXPECT_FALSE(ProgramShortname::Create("..").has_value());
  EXPECT_FALSE(ProgramShortname::Create("   ..   ").has_value());
}

TEST(ProgramShortname, LognameCorrect) {
  const std::map<std::string, std::string> name_to_logname = {
      // Does nothing.
      {"system", "system"},
      // Remove leading whitespace.
      {"    system", "system"},
      // Remove trailing whitespace.
      {"system    ", "system"},
      // Extracts components_for_foo
      {"bin/components_for_foo", "components_for_foo"},
      // Extracts foo_bar from the URL.
      {"fuchsia-pkg://fuchsia.com/foo-bar#meta/foo_bar.cm", "foo_bar"},
      // Extracts foo_bar from the URL.
      {"fuchsia.com:foo-bar#meta:foo_bar.cm", "foo_bar"},
  };

  for (const auto& [name, logname] : name_to_logname) {
    const std::optional<ProgramShortname> program_shortname = ProgramShortname::Create(name);
    ASSERT_TRUE(program_shortname.has_value());
    EXPECT_EQ(program_shortname->Logname(), logname);
  }
}

}  // namespace
}  // namespace forensics::crash_reports
