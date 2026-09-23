// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <re2/re2.h>

#include "tools/fidl/fidlc/src/diagnostic_types.h"
#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

using ::testing::HasSubstr;
using ::testing::Not;

// Reads a file into a string.
std::optional<std::string> ReadFile(const std::string& path) {
  std::ifstream input(path);
  if (!input) {
    return std::nullopt;
  }
  std::stringstream buf;
  buf << input.rdbuf();
  return buf.str();
}

// Returns all the good/fi-NNNN*.test.fidl paths found in _fi-NNNN.md.
std::optional<std::vector<std::string>> GoodFidlPaths(const DiagnosticDef& def) {
  auto id = def.FormatId();
  auto path = TestLibrary::TestFilePath("error-catalog/_" + id + ".md");
  auto content = ReadFile(path);
  if (!content) {
    return std::nullopt;
  }
  re2::StringPiece text = content.value();
  RE2 pattern =
      R"RE(gerrit_path="tools/fidl/fidlc/tests/fidl/(good/fi-(\d+)(?:-[a-z])?\.test.fidl)")RE";
  std::vector<std::string> result;
  std::string group1;
  ErrorId group2;
  while (RE2::FindAndConsume(&text, pattern, &group1, &group2)) {
    EXPECT_EQ(group2, def.id) << path << " references " << group1
                              << " which is for the wrong error ID";
    result.push_back(group1);
  }
  EXPECT_FALSE(result.empty()) << path << " does not reference any good .test.fidl files";
  return result;
}

TEST(ErrcatDocsTests, IndexIsComplete) {
  auto path = TestLibrary::TestFilePath("errcat.md");
  std::ifstream input(path);
  ASSERT_TRUE(input);
  auto it = std::begin(kAllDiagnosticDefs);
  const auto end = std::end(kAllDiagnosticDefs);
  std::string line;
  std::string prefix = "<<error-catalog/";
  while (it != end && std::getline(input, line)) {
    if (line.substr(0, prefix.size()) == prefix) {
      auto id = (*it)->FormatId();
      ASSERT_EQ(line, "<<error-catalog/_" + id + ".md>>")
          << "unexpected entry in " << path << "; either " << id
          << " was not next in sequence, or it is marked documented=false "
             "in diagnostics.h and should not appear";
      it = std::find_if(std::next(it), end,
                        [](const DiagnosticDef* def) { return def->opts.documented; });
    }
  }
  EXPECT_EQ(it, end) << path << " did not contain all diagnostics; missing " << (*it)->FormatId()
                     << " to " << (*std::prev(end))->FormatId();
}

TEST(ErrcatDocsTests, RedirectsAreComplete) {
  auto path = TestLibrary::TestFilePath("_redirects.yaml");
  auto redirects = ReadFile(path).value();
  for (auto def : kAllDiagnosticDefs) {
    auto id = def->FormatId();
    auto entry =
        "- from: /docs/error/" + id + "\n  to: /docs/reference/fidl/language/errcat.md#" + id;
    if (def->opts.documented) {
      EXPECT_THAT(redirects, HasSubstr(entry)) << path << " is missing a redirect for " << id;
    } else {
      EXPECT_THAT(redirects, Not(HasSubstr(entry)))
          << path << " unexpectedly has a redirect for " << id
          << ", which is marked documented=false in diagnostics.h";
    }
  }
}

TEST(ErrcatDocsTests, MarkdownFilesExist) {
  for (auto def : kAllDiagnosticDefs) {
    if (!def->opts.documented)
      continue;
    auto id = def->FormatId();
    auto path = TestLibrary::TestFilePath("error-catalog/_" + id + ".md");
    EXPECT_TRUE(std::filesystem::exists(path)) << "missing Markdown file " << path << " for " << id;
  }
}

TEST(ErrcatDocsTests, DocsAreAccurate) {
  for (auto def : kAllDiagnosticDefs) {
    if (!def->opts.documented) {
      continue;
    }
    auto id = def->FormatId();
    auto path = TestLibrary::TestFilePath("error-catalog/_" + id + ".md");
    auto content = ReadFile(path);
    if (!content) {
      // Will be caught by ErrcatDocsTests.MarkdownFilesExist.
      continue;
    }
    if (def->kind == DiagnosticKind::kRetired) {
      auto prefix = "## " + id + " {:#" + id + " .hide-from-toc}";
      EXPECT_EQ(content.value().substr(0, prefix.size()), prefix)
          << "first line of " << path << " is incorrect";
      EXPECT_THAT(content.value(), HasSubstr("Deprecated: This error code has been retired."))
          << id << " is DiagnosticKind::kRetired, "
          << "but the Markdown file does not say it is retired: " << path;
    } else {
      auto prefix = "## " + id + ":";
      EXPECT_EQ(content.value().substr(0, prefix.size()), prefix)
          << "first line of " << path << " is incorrect";
      EXPECT_THAT(content.value(), HasSubstr("{:#" + id + "}"))
          << "missing the expected heading id attribute in " << path;
    }
  }
}

TEST(ErrcatDocsTests, AllGoodFilesAreTested) {
  auto path = TestLibrary::TestFilePath("errcat_good_tests.cc");
  auto source_file = ReadFile(path).value();
  for (auto def : kAllDiagnosticDefs) {
    if (!def->opts.documented || def->kind == DiagnosticKind::kRetired) {
      // Will be caught by ErrorsAreTestedIffDocumentedAndNotRetired.
      continue;
    }
    if (def->id == ErrGeneratedZeroValueOrdinal.id) {
      // This error has no examples because it is impossible to test.
      continue;
    }
    auto fidl_paths = GoodFidlPaths(*def);
    if (!fidl_paths.has_value()) {
      // Will be caught by ErrcatDocsTests.MarkdownFilesExist.
      continue;
    }
    for (auto& fidl : fidl_paths.value()) {
      EXPECT_THAT(source_file, HasSubstr("\"" + fidl + "\""))
          << path << "does not contain a test for " << fidl;
    }
  }
}

TEST(ErrcatDocsTests, ErrorsAreTestedIffDocumentedAndNotRetired) {
  auto path = TestLibrary::TestFilePath("errcat_good_tests.cc");
  auto source_file = ReadFile(path).value();
  for (auto def : kAllDiagnosticDefs) {
    if (def->id == ErrGeneratedZeroValueOrdinal.id) {
      // This error has no examples because it is impossible to test.
      continue;
    }
    auto id = def->FormatId();
    bool tested = false;
    std::stringstream location;
    if (auto index = source_file.find("\"good/" + id); index != std::string::npos) {
      tested = true;
      location << path << ":"
               << 1 + std::count(source_file.data(), source_file.data() + index, '\n');
    }
    if (def->opts.documented && def->kind != DiagnosticKind::kRetired) {
      EXPECT_TRUE(tested) << id << " (documented=true, retired=false) is missing a test in "
                          << path;
    } else if (!def->opts.documented) {
      EXPECT_FALSE(tested) << id << " (documented=false) unexpectedly has a test at "
                           << location.str();
    } else {
      EXPECT_FALSE(tested) << id << " (retired=true) unexpectedly has a test at " << location.str();
    }
  }
}

}  // namespace
}  // namespace fidlc
