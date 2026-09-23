// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/diagnostics/reader/cpp/archive_reader.h>

#include <zxtest/zxtest.h>

namespace {

TEST(ArchiveReaderTest, SanitizeMoniker) {
  auto result = diagnostics::reader::SanitizeMonikerForSelectors("core/coll:bar/baz/other:foo");
  EXPECT_EQ(result, "core/coll\\:bar/baz/other\\:foo");
}

TEST(ArchiveReaderTest, MakeSelector) {
  {
    std::string expected = "core/simple:[name=\"root\"]root";
    std::string actual =
        diagnostics::reader::MakeSelector("core/simple", std::nullopt, {}, std::nullopt);
    EXPECT_EQ(actual, expected);
  }
  {
    std::string expected = "core/not\\:simple:[...]root/path:prop";
    std::string actual =
        diagnostics::reader::MakeSelector("core/not:simple", {{"..."}}, {"root", "path"}, "prop");
    EXPECT_EQ(actual, expected);
  }
  {
    std::string expected = "core/simple:[name=\"something messy\"]root/path:prop";
    std::string actual = diagnostics::reader::MakeSelector("core/simple", {{"something messy"}},
                                                           {"root", "path"}, "prop");
    EXPECT_EQ(actual, expected);
  }
  {
    std::string expected = "core/simple:[name=\"something\"]root/path";
    std::string actual = diagnostics::reader::MakeSelector("core/simple", {{"something"}},
                                                           {"root", "path"}, std::nullopt);
    EXPECT_EQ(actual, expected);
  }
  {
    std::string expected = "core/simple:[name=\"one\", name=\"two\", name=\"three\"]root/path:prop";
    std::string actual = diagnostics::reader::MakeSelector("core/simple", {{"one", "two", "three"}},
                                                           {"root", "path"}, "prop");
    EXPECT_EQ(actual, expected);
  }
  {
    std::string expected = "core/not\\:simple:[...]root/\\*:prop";
    std::string actual =
        diagnostics::reader::MakeSelector("core/not:simple", {{"..."}}, {"root", "\\*"}, "prop");
    EXPECT_EQ(actual, expected);
  }
  {
    std::string expected = "core/not\\:simple:[...]root/colons\\:are\\*messy:prop";
    std::string actual = diagnostics::reader::MakeSelector(
        "core/not:simple", {{"..."}}, {"root", "colons\\:are\\*messy"}, "prop");
    EXPECT_EQ(actual, expected);
  }
}

}  // namespace
